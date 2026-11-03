//! Exports real mixed videos and proves exactly what came out, frame by frame.

#[path = "../../bareclip-media/tests/common/mod.rs"]
mod common;

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use bareclip_core::{
    picture::Yuv420,
    placement::{Placement, WHOLE},
    splice::CutMode,
    time::Ticks,
    timeline::TimeRange,
};
use bareclip_export::{
    FullReport, LayeredClip, LayeredRequest, LayeredStretch, ProjectReport, export_layered,
    export_project,
};
use bareclip_media::{ExportPart, ExportRequest, MediaError, PictureReader, probe};
use common::*;

/// Colours are labelled, so frames of the canvas's size can skip the GPU.
const HD_LABELS: [&str; 6] = [
    "-colorspace",
    "bt709",
    "-color_primaries",
    "bt709",
    "-color_trc",
    "bt709",
];

/// Only one clip of each name gets made, however many tests run at once.
fn clip(name: &str, inputs: &[String]) -> PathBuf {
    static MADE: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    let made = MADE.get_or_init(|| Mutex::new(Vec::new()));
    let mut made = made.lock().unwrap_or_else(|poison| poison.into_inner());
    let path = output_dir().join(name);
    if !made.iter().any(|done| done == name) {
        let result = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(inputs)
            .arg(&path)
            .output()
            .expect("ffmpeg is on PATH");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        made.push(name.to_owned());
    }
    path
}

fn to_strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// Sixteen blocks across the picture spell its frame number, plus `first`.
fn numbered_video(size: (u32, u32), rate: u32, seconds: u32, first: u32) -> String {
    let (width, height) = size;
    let block = width / 16;
    format!(
        "nullsrc=s={width}x{height}:r={rate}:d={seconds},\
         geq=lum='255*mod(floor((N+{first})/pow(2,floor(X/{block}))),2)':cb=128:cr=128,format=yuv420p"
    )
}

fn beeps(seconds: u32) -> String {
    format!("aevalsrc='0.8*sin(2*PI*440*t)*lt(mod(t,1),0.1)':d={seconds}:s=48000")
}

fn tone(frequency: u32, seconds: u32) -> String {
    format!("sine=frequency={frequency}:sample_rate=48000:duration={seconds}")
}

fn encoded(video: String, sound: &[String]) -> Vec<String> {
    let mut args = to_strings(&["-f", "lavfi", "-i"]);
    args.push(video);
    for source in sound {
        args.extend(to_strings(&["-f", "lavfi", "-i"]));
        args.push(source.clone());
    }
    for index in 0..=sound.len() {
        args.push("-map".to_owned());
        args.push(index.to_string());
    }
    args.extend(to_strings(&[
        "-c:v",
        "libopenh264",
        "-b:v",
        "4M",
        "-g",
        "30",
        "-c:a",
        "aac",
    ]));
    args.extend(to_strings(&HD_LABELS));
    args
}

/// The main test clip: 320 x 180 at 30 fps, numbered from 0, with beeps on each second.
fn large_clip() -> PathBuf {
    clip(
        "full-large.mp4",
        &encoded(numbered_video((320, 180), 30, 4, 0), &[beeps(4)]),
    )
}

/// The same shape at half the size, numbered from 1000.
fn small_clip() -> PathBuf {
    clip(
        "full-small.mp4",
        &encoded(numbered_video((160, 90), 30, 2, 1000), &[beeps(2)]),
    )
}

/// The same size as the main clip at twice the rate, numbered from 2000.
fn fast_clip() -> PathBuf {
    clip(
        "full-60.mp4",
        &encoded(numbered_video((320, 180), 60, 2, 2000), &[beeps(2)]),
    )
}

fn part(path: &Path, start: f64, end: f64, gap: f64) -> ExportPart {
    ExportPart {
        input: path.to_owned(),
        frames: Arc::new(index_of(path)),
        range: TimeRange {
            start: seconds(start),
            end: seconds(end),
        },
        gap_before: seconds(gap),
    }
}

fn request(name: &str, parts: Vec<ExportPart>) -> ExportRequest {
    ExportRequest {
        output: output_path(name, "mp4"),
        parts,
        mode: CutMode::FrameExact,
    }
}

fn run(request: &ExportRequest) -> ProjectReport {
    export_project(request, &AtomicBool::new(false), &|_| {}).expect("the export succeeds")
}

fn run_full(request: &ExportRequest) -> FullReport {
    match run(request) {
        ProjectReport::Full(report) => report,
        ProjectReport::Fast(report) => panic!("it took the fast route: {report:?}"),
    }
}

/// What a picture of the finished video shows, read at the size it came out.
#[derive(Clone, Copy, Debug)]
struct Seen {
    seconds: f64,
    number: u32,
    middle: u8,
}

fn brightness_at(view: &Yuv420, x: usize, y: usize) -> u8 {
    view.y[y * view.y_stride + x]
}

fn number_in(view: &Yuv420) -> u32 {
    let block = view.width as usize / 16;
    let middle = view.height as usize / 2;
    (0..16).fold(0, |number, bit| {
        if brightness_at(view, bit * block + block / 2, middle) > 128 {
            number | 1 << bit
        } else {
            number
        }
    })
}

fn pictures_of(path: &Path) -> Vec<Seen> {
    let mut reader = PictureReader::open(path).expect("the export opens");
    let mut seen = Vec::new();
    while let Some(picture) = reader.next_picture().expect("it decodes") {
        let view = picture.view();
        seen.push(Seen {
            seconds: picture.time().as_seconds(),
            number: number_in(&view),
            middle: brightness_at(&view, view.width as usize / 2, view.height as usize / 2),
        });
    }
    seen
}

/// The names of a file's sound tracks, from where MP4 keeps them.
fn track_names(path: &Path) -> Vec<String> {
    let input = ffmpeg_next::format::input(path).expect("the export opens");
    input
        .streams()
        .filter(|stream| stream.parameters().medium() == ffmpeg_next::media::Type::Audio)
        .filter_map(|stream| stream.metadata().get("handler_name").map(str::to_owned))
        .collect()
}

fn expected_run(frames: std::ops::Range<u32>) -> Vec<u32> {
    frames.collect()
}

#[test]
fn videos_that_match_stay_on_the_fast_route() {
    let job = request(
        "matching",
        vec![
            part(&large_clip(), 0.0, 1.0, 0.0),
            part(&large_clip(), 2.0, 3.0, 0.0),
        ],
    );

    let ProjectReport::Fast(report) = run(&job) else {
        panic!("copying was possible");
    };

    assert_eq!(report.sections, 2);
}

#[test]
fn a_smaller_video_is_scaled_up_and_joined_to_a_larger_one() {
    let job = request(
        "mixed-size",
        vec![
            part(&large_clip(), 0.0, 1.0, 0.0),
            part(&small_clip(), 0.0, 1.0, 0.0),
        ],
    );

    let report = run_full(&job);

    let info = probe(&job.output).expect("the export can be read");
    assert_eq!((info.video.width, info.video.height), (320, 180));
    assert_eq!(info.video.codec, "h264");
    assert_eq!(report.frames, 60);
    let mut wanted = expected_run(0..30);
    wanted.extend(1000..1030);
    let found: Vec<u32> = pictures_of(&job.output)
        .iter()
        .map(|seen| seen.number)
        .collect();
    assert_eq!(found, wanted);
}

#[test]
fn only_frames_that_need_it_go_through_the_gpu() {
    let job = request(
        "gpu-only-when-needed",
        vec![
            part(&large_clip(), 0.0, 1.0, 0.0),
            part(&small_clip(), 0.0, 0.5, 0.0),
        ],
    );

    let report = run_full(&job);

    assert_eq!(report.direct_frames, 30, "the large clip fills the canvas");
    assert_eq!(
        report.composited_frames, 15,
        "the small clip must be scaled"
    );
    assert!(report.gpu.is_some());
}

#[test]
fn the_gpu_is_never_opened_if_no_frame_needs_it() {
    let job = request(
        "no-gpu",
        vec![
            part(&large_clip(), 0.0, 1.0, 0.0),
            part(&fast_clip(), 0.0, 1.0, 0.0),
        ],
    );

    let report = run_full(&job);

    assert_eq!(report.composited_frames, 0);
    assert_eq!(report.gpu, None);
}

#[test]
fn different_frame_rates_become_the_highest_kept_constant() {
    let job = request(
        "mixed-rate",
        vec![
            part(&large_clip(), 0.0, 1.0, 0.0),
            part(&fast_clip(), 0.0, 1.0, 0.0),
        ],
    );

    let report = run_full(&job);

    assert_eq!(report.frame_rate, 60.0);
    assert_eq!(report.frames, 120);
    assert_eq!(
        report.repeated_frames, 30,
        "the slower clip repeats its frames"
    );
    let seen = pictures_of(&job.output);
    assert_eq!(seen.len(), 120);
    for (index, picture) in seen.iter().enumerate() {
        let wanted = index as f64 / 60.0;
        assert!(
            (picture.seconds - wanted).abs() < 0.002,
            "frame {index}: {picture:?}"
        );
    }
    let mut wanted: Vec<u32> = (0..30).flat_map(|number| [number, number]).collect();
    wanted.extend(2000..2060);
    let found: Vec<u32> = seen.iter().map(|picture| picture.number).collect();
    assert_eq!(found, wanted);
}

/// Matroska rounds times to the millisecond, as in an OBS recording, so a frame can land a hair
/// after its slot. Taking "the latest frame so far" there would skip it and repeat the last.
#[test]
fn rounded_timestamps_never_make_frames_stutter() {
    let rounded = clip("full-60.mkv", &{
        let mut args = encoded(numbered_video((320, 180), 60, 2, 0), &[]);
        args.retain(|arg| arg != "-c:a" && arg != "aac");
        args
    });
    let job = request(
        "rounded",
        vec![
            part(&rounded, 0.0, 1.0, 0.0),
            part(&small_clip(), 0.0, 0.5, 0.0),
        ],
    );

    let report = run_full(&job);

    assert_eq!(report.frames, 90);
    let found: Vec<u32> = pictures_of(&job.output)
        .iter()
        .map(|seen| seen.number)
        .take(60)
        .collect();
    assert_eq!(found, expected_run(0..60), "every frame once, in order");
}

#[test]
fn a_cut_lands_on_the_exact_frame_even_between_keyframes() {
    let job = request(
        "exact-start",
        vec![
            part(&large_clip(), 1.5, 2.5, 0.0),
            part(&small_clip(), 0.0, 0.5, 0.0),
        ],
    );

    run_full(&job);

    let found: Vec<u32> = pictures_of(&job.output)
        .iter()
        .map(|seen| seen.number)
        .collect();
    let mut wanted = expected_run(45..75);
    wanted.extend(1000..1015);
    assert_eq!(found, wanted);
}

#[test]
fn dead_space_becomes_black_and_silent_for_the_right_length() {
    let job = request(
        "gap",
        vec![
            part(&large_clip(), 0.0, 1.0, 0.0),
            part(&small_clip(), 0.0, 1.0, 1.0),
        ],
    );

    let report = run_full(&job);

    assert_eq!(report.frames, 90);
    assert_eq!(report.black_frames, 30);
    let seen = pictures_of(&job.output);
    assert!(seen[29].number == 29, "{:?}", seen[29]);
    assert!(seen[30..60].iter().all(|picture| picture.number == 0));
    assert!(seen[45].middle < 30, "it is black: {:?}", seen[45]);
    assert_eq!(seen[60].number, 1000);

    let sound = decode_audio(&job.output);
    let starts = beep_times(&sound[0]);
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert!(starts[0].abs() < 0.05, "{starts:?}");
    assert!((starts[1] - 2.0).abs() < 0.05, "{starts:?}");
}

#[test]
fn sound_stays_in_sync_after_joining_and_cutting() {
    // Cutting into the second second means its beep is at the very start.
    let job = request(
        "sync",
        vec![
            part(&large_clip(), 1.0, 3.0, 0.0),
            part(&small_clip(), 0.0, 2.0, 0.0),
        ],
    );

    run_full(&job);

    let sound = decode_audio(&job.output);
    let starts = beep_times(&sound[0]);
    assert_eq!(starts.len(), 4, "{starts:?}");
    for (found, wanted) in starts.iter().zip([0.0, 1.0, 2.0, 3.0]) {
        assert!((found - wanted).abs() < 0.05, "{starts:?}");
    }
    assert!((sound[0].seconds - 4.0).abs() < 0.1, "{}", sound[0].seconds);
}

#[test]
fn sound_tracks_are_kept_apart_with_silence_where_a_video_has_fewer() {
    let two_tracks = clip("full-two-tracks.mkv", &{
        let mut args = encoded(
            numbered_video((320, 180), 30, 2, 3000),
            &[tone(440, 2), tone(1760, 2)],
        );
        args.extend(to_strings(&[
            "-metadata:s:a:0",
            "title=Game",
            "-metadata:s:a:1",
            "title=Microphone",
        ]));
        args
    });
    let job = request(
        "tracks",
        vec![
            part(&small_clip(), 0.0, 1.0, 0.0),
            part(&two_tracks, 0.0, 1.0, 0.0),
        ],
    );

    let report = run_full(&job);

    assert_eq!(report.audio_tracks, 2);
    let sound = decode_audio(&job.output);
    assert_eq!(sound.len(), 2);
    assert_eq!(track_names(&job.output), ["Game", "Microphone"]);
    let rate = sound[1].rate;
    let second = rate as usize;
    let quiet = &sound[1].samples[second / 4..second * 3 / 4];
    assert!(
        quiet.iter().all(|sample| sample.abs() < 0.01),
        "only silence"
    );
    let pitch = pitch_of(
        &sound[1].samples[second + second / 4..second + second * 3 / 4],
        rate,
    );
    assert!((pitch - 1760.0).abs() < 30.0, "{pitch}");
    let pitch = pitch_of(
        &sound[0].samples[second + second / 4..second + second * 3 / 4],
        rate,
    );
    assert!((pitch - 440.0).abs() < 30.0, "{pitch}");
}

#[test]
fn a_video_with_no_sound_is_silent_among_others_that_have_it() {
    let job = request(
        "silent-part",
        vec![
            part(&silent_mp4(), 0.0, 1.0, 0.0),
            part(&small_clip(), 0.0, 1.0, 0.0),
        ],
    );

    run_full(&job);

    let sound = decode_audio(&job.output);
    let starts = beep_times(&sound[0]);
    assert_eq!(starts.len(), 1, "{starts:?}");
    assert!((starts[0] - 1.0).abs() < 0.05, "{starts:?}");
}

#[test]
fn no_video_having_sound_makes_a_file_with_no_sound() {
    let quiet_small = clip(
        "full-quiet-small.mp4",
        &encoded(numbered_video((160, 90), 30, 2, 0), &[]),
    );
    let job = request(
        "no-sound",
        vec![
            part(&silent_mp4(), 0.0, 0.5, 0.0),
            part(&quiet_small, 0.0, 0.5, 0.0),
        ],
    );

    let report = run_full(&job);

    assert_eq!(report.audio_tracks, 0);
    assert!(probe(&job.output).expect("it can be read").audio.is_empty());
}

#[test]
fn a_tall_video_keeps_all_of_itself_with_black_bars_at_the_sides() {
    let tall = clip(
        "full-tall.mp4",
        &encoded(
            "color=c=white:s=90x160:r=30:d=1,format=yuv420p".to_owned(),
            &[],
        ),
    );
    let job = request(
        "bars",
        vec![
            part(&large_clip(), 0.0, 0.5, 0.0),
            part(&tall, 0.0, 0.5, 0.0),
        ],
    );

    run_full(&job);

    let mut reader = PictureReader::open(&job.output).expect("the export opens");
    let mut last = None;
    while let Some(picture) = reader.next_picture().expect("it decodes") {
        last = Some(picture);
    }
    let last = last.expect("it has pictures");
    let view = last.view();
    // 90 x 160 fits 320 x 180 as about 101 wide in the middle.
    assert!(
        brightness_at(&view, 160, 10) > 200,
        "the top of the picture is there"
    );
    assert!(
        brightness_at(&view, 160, 170) > 200,
        "the bottom of the picture is there"
    );
    assert!(brightness_at(&view, 60, 90) < 30, "a bar on the left");
    assert!(brightness_at(&view, 260, 90) < 30, "a bar on the right");
}

#[test]
fn colours_look_the_same_whichever_way_the_frame_goes() {
    // Each frame's colour must survive both the direct way and the GPU.
    let flat = "color=c=0x3080C0:s=320x180:r=30:d=1,\
                scale=out_color_matrix=bt709:out_range=tv,format=yuv420p";
    let large = clip("full-colour.mp4", &encoded(flat.to_owned(), &[]));
    let small = clip(
        "full-colour-small.mp4",
        &encoded(flat.replace("320x180", "160x90"), &[]),
    );
    let job = request(
        "colour",
        vec![part(&large, 0.0, 0.5, 0.0), part(&small, 0.0, 0.5, 0.0)],
    );

    let report = run_full(&job);
    assert!(report.direct_frames > 0 && report.composited_frames > 0);

    let mut reader = PictureReader::open(&job.output).expect("the export opens");
    let (mut lowest, mut highest) = ((255, 255, 255), (0, 0, 0));
    while let Some(picture) = reader.next_picture().expect("it decodes") {
        let view = picture.view();
        let y = i32::from(brightness_at(&view, 160, 90));
        let u = i32::from(view.u[45 * view.uv_stride + 80]);
        let v = i32::from(view.v[45 * view.uv_stride + 80]);
        lowest = (lowest.0.min(y), lowest.1.min(u), lowest.2.min(v));
        highest = (highest.0.max(y), highest.1.max(u), highest.2.max(v));
    }
    assert!(highest.0 - lowest.0 <= 4, "{lowest:?} to {highest:?}");
    assert!(highest.1 - lowest.1 <= 4, "{lowest:?} to {highest:?}");
    assert!(highest.2 - lowest.2 <= 4, "{lowest:?} to {highest:?}");
}

#[test]
fn old_formats_and_new_ones_can_be_joined() {
    let job = request(
        "codecs",
        vec![
            part(&h264_mp4(), 0.0, 0.5, 0.0),
            part(&hevc_mkv(), 0.0, 0.5, 0.0),
            part(&mpeg4_mkv(), 0.0, 0.5, 0.0),
            part(&av1_mkv(), 0.0, 0.5, 0.0),
        ],
    );

    let report = run_full(&job);

    assert_eq!(report.frames, 60);
    let info = probe(&job.output).expect("it can be read");
    assert_eq!(info.video.codec, "h264");
    assert_eq!((info.video.width, info.video.height), (320, 184));
    assert_eq!(pictures_of(&job.output).len(), 60);
}

#[test]
fn dead_space_in_a_format_that_cant_be_copied_now_exports() {
    let job = request("old-gap", vec![part(&mpeg4_mkv(), 0.0, 0.5, 0.5)]);

    let report = run_full(&job);

    assert_eq!(report.frames, 30);
    assert_eq!(report.black_frames, 15);
}

#[test]
fn a_recording_with_uneven_timing_comes_out_even() {
    let job = request(
        "uneven",
        vec![
            part(&variable_rate_mkv(), 1.0, 3.0, 0.0),
            part(&small_clip(), 0.0, 0.5, 0.0),
        ],
    );

    let report = run_full(&job);

    let seen = pictures_of(&job.output);
    assert_eq!(seen.len() as u64, report.frames);
    let frame = 1.0 / report.frame_rate;
    for (index, picture) in seen.iter().enumerate() {
        let wanted = index as f64 * frame;
        assert!(
            (picture.seconds - wanted).abs() < 0.002,
            "frame {index}: {picture:?}"
        );
    }
    let numbers: Vec<u32> = seen.iter().map(|picture| picture.number).collect();
    assert!(
        numbers[..60].windows(2).all(|pair| pair[0] <= pair[1]),
        "time never runs backwards: {numbers:?}"
    );
}

#[test]
fn an_odd_sized_video_rounds_up_to_an_even_canvas() {
    // H.264 encoders here round odd sizes away, and this format keeps them.
    let odd = clip(
        "full-odd.mkv",
        &to_strings(&[
            "-f",
            "lavfi",
            "-i",
            "color=c=white:s=321x181:r=30:d=1,format=yuv420p",
            "-c:v",
            "mpeg4",
        ]),
    );
    assert_eq!(probe(&odd).expect("it can be read").video.width, 321);
    let job = request(
        "odd",
        vec![
            part(&large_clip(), 0.0, 0.5, 0.0),
            part(&odd, 0.0, 0.5, 0.0),
        ],
    );

    run_full(&job);

    let info = probe(&job.output).expect("it can be read");
    assert_eq!((info.video.width, info.video.height), (322, 182));
}

#[test]
fn the_report_says_how_long_and_what_encoded_it() {
    let job = request(
        "report",
        vec![
            part(&large_clip(), 0.0, 0.5, 0.0),
            part(&small_clip(), 0.0, 0.5, 0.0),
        ],
    );

    let report = run_full(&job);

    assert!(report.encoder.starts_with("h264_") || report.encoder == "libopenh264");
    assert!(report.elapsed.as_nanos() > 0);
    assert_eq!((report.width, report.height), (320, 180));
}

#[test]
fn progress_reaches_the_end() {
    let job = request(
        "progress",
        vec![
            part(&large_clip(), 0.0, 0.5, 0.0),
            part(&small_clip(), 0.0, 0.5, 0.0),
        ],
    );
    let last = Mutex::new(0.0_f32);

    export_project(&job, &AtomicBool::new(false), &|fraction| {
        *last.lock().expect("it is free") = fraction;
    })
    .expect("the export succeeds");

    assert_eq!(*last.lock().expect("it is free"), 1.0);
}

#[test]
fn cancelling_stops_the_export_and_leaves_nothing_behind() {
    let job = request(
        "cancelled",
        vec![
            part(&large_clip(), 0.0, 3.0, 0.0),
            part(&small_clip(), 0.0, 1.0, 0.0),
        ],
    );
    let cancel = AtomicBool::new(false);

    let result = export_project(&job, &cancel, &|fraction| {
        if fraction > 0.2 {
            cancel.store(true, Ordering::Relaxed);
        }
    });

    assert!(matches!(result, Err(MediaError::Cancelled)));
    assert!(!job.output.exists());
    let partial = job.output.with_file_name("out-cancelled.partial.mp4");
    assert!(!partial.exists(), "the unfinished file was cleaned up");
}

#[test]
fn exporting_over_a_source_video_is_refused() {
    let source = clip(
        "full-overwrite.mp4",
        &encoded(numbered_video((320, 180), 30, 1, 0), &[]),
    );
    let job = ExportRequest {
        output: source.clone(),
        parts: vec![
            part(&source, 0.0, 0.5, 0.0),
            part(&small_clip(), 0.0, 0.5, 0.0),
        ],
        mode: CutMode::FrameExact,
    };

    let result = export_project(&job, &AtomicBool::new(false), &|_| {});

    assert!(matches!(result, Err(MediaError::SameInputAndOutput)));
    assert!(source.exists());
}

#[test]
fn the_length_never_drifts_from_the_timeline() {
    let job = request(
        "length",
        vec![
            part(&large_clip(), 0.25, 1.75, 0.0),
            part(&small_clip(), 0.5, 1.5, 0.25),
        ],
    );

    let report = run_full(&job);

    assert_eq!(report.frames, 83, "2.75 seconds at 30 fps, rounded");
    let info = probe(&job.output).expect("it can be read");
    let off = (info.duration.as_seconds() - 2.75).abs();
    assert!(off < 0.1, "{}", info.duration.as_seconds());
}

/// A flat colour for layers to be told apart by, labelled so it can skip the GPU alone.
fn flat_clip(name: &str, colour: &str, size: (u32, u32), seconds: u32) -> PathBuf {
    let video = format!(
        "color=c={colour}:s={}x{}:r=30:d={seconds},format=yuv420p",
        size.0, size.1
    );
    clip(name, &encoded(video, &[beeps(seconds)]))
}

fn red_clip() -> PathBuf {
    flat_clip("layer-red.mp4", "red", (320, 180), 2)
}

fn blue_clip() -> PathBuf {
    flat_clip("layer-blue.mp4", "blue", (320, 180), 2)
}

fn whole(input: &Path, key: u64, start: f64, length: f64, placement: Placement) -> LayeredClip {
    LayeredClip {
        key,
        input: input.to_owned(),
        start: seconds(start),
        source: TimeRange {
            start: Ticks(0),
            end: seconds(length),
        },
        placement,
    }
}

fn layered_request(
    name: &str,
    stretches: Vec<LayeredStretch>,
    sound: Vec<ExportPart>,
) -> LayeredRequest {
    LayeredRequest {
        output: output_path(name, "mp4"),
        stretches,
        sound,
        canvas: None,
        background: [0, 0, 0],
    }
}

fn stretch(start: f64, end: f64, stack: Vec<LayeredClip>) -> LayeredStretch {
    LayeredStretch {
        start: seconds(start),
        end: seconds(end),
        stack,
    }
}

fn run_layered(request: &LayeredRequest) -> FullReport {
    export_layered(request, &AtomicBool::new(false), &|_| {}).expect("the export succeeds")
}

fn colour_at(view: &Yuv420, x: usize, y: usize) -> (u8, u8) {
    (
        view.u[y / 2 * view.uv_stride + x / 2],
        view.v[y / 2 * view.uv_stride + x / 2],
    )
}

fn is_red(colour: (u8, u8)) -> bool {
    colour.1 > 200 && colour.0 < 128
}

fn is_blue(colour: (u8, u8)) -> bool {
    colour.0 > 200 && colour.1 < 128
}

/// Half the size, in the canvas's top left quarter.
fn top_left_quarter() -> Placement {
    Placement {
        center_x: WHOLE / 4,
        center_y: WHOLE / 4,
        scale: WHOLE / 2,
        ..Placement::default()
    }
}

#[test]
fn a_resized_clip_shows_the_one_below_around_it_on_every_frame() {
    let job = layered_request(
        "layer-two",
        vec![stretch(
            0.0,
            2.0,
            vec![
                whole(&red_clip(), 1, 0.0, 2.0, Placement::default()),
                whole(&blue_clip(), 2, 0.0, 2.0, top_left_quarter()),
            ],
        )],
        vec![part(&blue_clip(), 0.0, 2.0, 0.0)],
    );

    let report = run_layered(&job);

    assert_eq!((report.width, report.height), (320, 180));
    assert_eq!(report.frames, 60);
    assert!(report.gpu.is_some());
    let mut reader = PictureReader::open(&job.output).expect("the export opens");
    let mut count = 0;
    while let Some(picture) = reader.next_picture().expect("it decodes") {
        let view = picture.view();
        assert!(is_blue(colour_at(&view, 80, 45)), "inside {count}");
        assert!(is_red(colour_at(&view, 240, 45)), "right of it {count}");
        assert!(is_red(colour_at(&view, 80, 135)), "below it {count}");
        count += 1;
    }
    assert_eq!(count, 60);
}

#[test]
fn a_clip_moved_partly_off_the_canvas_shows_only_what_is_on_it() {
    let job = layered_request(
        "layer-off",
        vec![stretch(
            0.0,
            1.0,
            vec![
                whole(&red_clip(), 1, 0.0, 2.0, Placement::default()),
                whole(
                    &blue_clip(),
                    2,
                    0.0,
                    2.0,
                    Placement {
                        center_x: 0,
                        center_y: 0,
                        ..top_left_quarter()
                    },
                ),
            ],
        )],
        Vec::new(),
    );

    run_layered(&job);

    let mut reader = PictureReader::open(&job.output).expect("the export opens");
    let picture = reader
        .next_picture()
        .expect("it decodes")
        .expect("a picture");
    let view = picture.view();
    assert!(is_blue(colour_at(&view, 40, 20)));
    assert!(is_red(colour_at(&view, 120, 20)));
    assert!(is_red(colour_at(&view, 40, 90)));
}

#[test]
fn a_see_through_clip_mixes_with_the_one_below() {
    let job = layered_request(
        "layer-see-through",
        vec![stretch(
            0.0,
            1.0,
            vec![
                whole(&red_clip(), 1, 0.0, 2.0, Placement::default()),
                whole(
                    &blue_clip(),
                    2,
                    0.0,
                    2.0,
                    Placement {
                        opacity: WHOLE / 2,
                        ..Placement::default()
                    },
                ),
            ],
        )],
        Vec::new(),
    );

    run_layered(&job);

    let mut reader = PictureReader::open(&job.output).expect("the export opens");
    let picture = reader
        .next_picture()
        .expect("it decodes")
        .expect("a picture");
    let (u, v) = colour_at(&picture.view(), 160, 90);
    assert!(!is_red((u, v)) && !is_blue((u, v)), "{u} {v}");
    assert!(u > 150 && v > 150, "purple has both: {u} {v}");
}

#[test]
fn an_overlay_comes_and_goes_on_the_exact_frame_and_footage_carries_on_under_it() {
    let large = large_clip();
    let job = layered_request(
        "layer-comes-and-goes",
        vec![
            stretch(
                0.0,
                1.0,
                vec![whole(&large, 1, 0.0, 3.0, Placement::default())],
            ),
            stretch(
                1.0,
                2.0,
                vec![
                    whole(&large, 1, 0.0, 3.0, Placement::default()),
                    whole(
                        &blue_clip(),
                        2,
                        1.0,
                        1.0,
                        Placement {
                            center_x: WHOLE / 4,
                            center_y: WHOLE / 8,
                            scale: WHOLE / 4,
                            opacity: WHOLE,
                            ..Placement::default()
                        },
                    ),
                ],
            ),
            stretch(
                2.0,
                3.0,
                vec![whole(&large, 1, 0.0, 3.0, Placement::default())],
            ),
        ],
        Vec::new(),
    );

    let report = run_layered(&job);

    // The blocks spelling the frame number run along the middle, clear of the overlay.
    let seen = pictures_of(&job.output);
    let numbers: Vec<u32> = seen.iter().map(|picture| picture.number).collect();
    assert_eq!(numbers, expected_run(0..90));
    let mut reader = PictureReader::open(&job.output).expect("the export opens");
    let mut frame = 0;
    while let Some(picture) = reader.next_picture().expect("it decodes") {
        let overlaid = is_blue(colour_at(&picture.view(), 80, 22));
        assert_eq!(overlaid, (30..60).contains(&frame), "frame {frame}");
        frame += 1;
    }
    // Only the frames with the overlay needed the GPU.
    assert_eq!(report.composited_frames, 30);
    assert_eq!(report.direct_frames, 60);
}

#[test]
fn a_clip_alone_in_its_normal_place_never_opens_the_gpu() {
    let job = layered_request(
        "layer-plain",
        vec![stretch(
            0.0,
            1.0,
            vec![whole(&large_clip(), 1, 0.0, 1.0, Placement::default())],
        )],
        Vec::new(),
    );

    let report = run_layered(&job);

    assert_eq!(report.gpu, None);
    assert_eq!(report.composited_frames, 0);
}

#[test]
fn frames_where_nothing_changes_are_repeated_rather_than_drawn() {
    let job = layered_request(
        "layer-repeats",
        vec![stretch(
            0.0,
            1.0,
            vec![
                whole(&large_clip(), 1, 0.0, 1.0, Placement::default()),
                whole(&blue_clip(), 2, 0.0, 1.0, top_left_quarter()),
            ],
        )],
        Vec::new(),
    );

    let report = run_layered(&job);

    // At twice the rate of what it shows, about half are just shown again.
    assert_eq!(report.frames, 30);
    assert_eq!(report.composited_frames + report.repeated_frames, 30);
    assert!(report.composited_frames >= 29, "{report:?}");
}

#[test]
fn dead_space_between_layers_is_black_and_keeps_its_length() {
    let job = layered_request(
        "layer-gap",
        vec![
            stretch(
                0.0,
                1.0,
                vec![whole(&red_clip(), 1, 0.0, 1.0, top_left_quarter())],
            ),
            stretch(
                2.0,
                3.0,
                vec![whole(&blue_clip(), 2, 2.0, 1.0, top_left_quarter())],
            ),
        ],
        Vec::new(),
    );

    let report = run_layered(&job);

    assert_eq!(report.frames, 90);
    let seen = pictures_of(&job.output);
    assert_eq!(seen.len(), 90);
    assert!(seen[45].middle < 30, "{:?}", seen[45]);
}

#[test]
fn a_larger_video_under_a_smaller_one_sets_the_size_and_nothing_is_cropped() {
    let job = layered_request(
        "layer-sizes",
        vec![stretch(
            0.0,
            1.0,
            vec![
                whole(&large_clip(), 1, 0.0, 1.0, Placement::default()),
                whole(&small_clip(), 2, 0.0, 1.0, top_left_quarter()),
            ],
        )],
        Vec::new(),
    );

    let report = run_layered(&job);

    assert_eq!((report.width, report.height), (320, 180));
}

#[test]
fn sound_comes_from_the_top_clip_and_stays_in_sync_under_layers() {
    let job = layered_request(
        "layer-sync",
        vec![stretch(
            0.0,
            2.0,
            vec![
                whole(&red_clip(), 1, 0.0, 2.0, Placement::default()),
                whole(&large_clip(), 2, 0.0, 2.0, top_left_quarter()),
            ],
        )],
        vec![part(&large_clip(), 0.0, 2.0, 0.0)],
    );

    run_layered(&job);

    let sound = decode_audio(&job.output);
    let starts = beep_times(&sound[0]);
    assert_eq!(starts.len(), 2, "{starts:?}");
    for (found, wanted) in starts.iter().zip([0.0, 1.0]) {
        assert!((found - wanted).abs() < 0.05, "{starts:?}");
    }
}

#[test]
fn a_layered_export_can_be_cancelled_and_leaves_nothing_behind() {
    let job = layered_request(
        "layer-cancel",
        vec![stretch(
            0.0,
            2.0,
            vec![whole(&red_clip(), 1, 0.0, 2.0, top_left_quarter())],
        )],
        Vec::new(),
    );

    let result = export_layered(&job, &AtomicBool::new(true), &|_| {});

    assert!(matches!(result, Err(MediaError::Cancelled)));
    assert!(!job.output.exists());
}

#[test]
fn a_layered_export_over_one_of_its_videos_is_refused() {
    let job = LayeredRequest {
        output: red_clip(),
        stretches: vec![stretch(
            0.0,
            1.0,
            vec![whole(&red_clip(), 1, 0.0, 1.0, top_left_quarter())],
        )],
        sound: Vec::new(),
        canvas: None,
        background: [0, 0, 0],
    };

    let result = export_layered(&job, &AtomicBool::new(false), &|_| {});

    assert!(matches!(result, Err(MediaError::SameInputAndOutput)));
}

#[test]
fn a_size_the_project_chose_is_what_comes_out_with_bars_around_the_picture() {
    let mut job = layered_request(
        "layer-chosen-size",
        vec![stretch(
            0.0,
            1.0,
            vec![whole(&red_clip(), 1, 0.0, 1.0, Placement::default())],
        )],
        Vec::new(),
    );
    job.canvas = Some((180, 320));

    let report = run_layered(&job);

    assert_eq!((report.width, report.height), (180, 320));
    let mut reader = PictureReader::open(&job.output).expect("the export opens");
    let picture = reader
        .next_picture()
        .expect("it decodes")
        .expect("a picture");
    let view = picture.view();
    assert_eq!((view.width, view.height), (180, 320));
    assert!(
        is_red(colour_at(&view, 90, 160)),
        "the picture is in the middle"
    );
    assert!(
        brightness_at(&view, 90, 10) < 30,
        "with a black bar above it"
    );
    assert!(brightness_at(&view, 90, 310) < 30, "and below it");
}

#[test]
fn a_larger_video_is_scaled_down_to_a_smaller_project() {
    let mut job = layered_request(
        "layer-scaled-down",
        vec![stretch(
            0.0,
            1.0,
            vec![whole(&red_clip(), 1, 0.0, 1.0, Placement::default())],
        )],
        Vec::new(),
    );
    job.canvas = Some((160, 90));

    let report = run_layered(&job);

    assert_eq!((report.width, report.height), (160, 90));
    assert!(report.composited_frames > 0, "fitting it takes the GPU");
}

#[test]
fn a_clip_turned_a_quarter_stands_upright_with_bars_at_its_sides() {
    let job = layered_request(
        "layer-turned",
        vec![stretch(
            0.0,
            1.0,
            vec![whole(
                &red_clip(),
                1,
                0.0,
                1.0,
                Placement::default().turned_quarter((320, 180), (320, 180)),
            )],
        )],
        Vec::new(),
    );

    let report = run_layered(&job);

    assert_eq!((report.width, report.height), (320, 180));
    let mut reader = PictureReader::open(&job.output).expect("the export opens");
    let picture = reader
        .next_picture()
        .expect("it decodes")
        .expect("a picture");
    let view = picture.view();
    assert!(is_red(colour_at(&view, 160, 90)), "it is in the middle");
    assert!(brightness_at(&view, 40, 90) < 30, "with a bar on the left");
    assert!(brightness_at(&view, 280, 90) < 30, "and on the right");
}

#[test]
fn the_background_colour_fills_bars_and_dead_space_and_keeps_its_length() {
    let mut job = layered_request(
        "layer-background",
        vec![
            stretch(
                0.0,
                1.0,
                vec![whole(&red_clip(), 1, 0.0, 1.0, top_left_quarter())],
            ),
            stretch(
                2.0,
                3.0,
                vec![whole(&red_clip(), 2, 2.0, 1.0, Placement::default())],
            ),
        ],
        Vec::new(),
    );
    job.background = [0, 0, 255];

    let report = run_layered(&job);

    assert_eq!(report.frames, 90);
    let mut reader = PictureReader::open(&job.output).expect("the export opens");
    let mut frame = 0;
    while let Some(picture) = reader.next_picture().expect("it decodes") {
        let view = picture.view();
        let (corner, far_side) = (colour_at(&view, 80, 45), colour_at(&view, 240, 135));
        match frame {
            0..30 => {
                assert!(is_red(corner), "the picture at {frame}");
                assert!(is_blue(far_side), "the background around it at {frame}");
            }
            30..60 => {
                assert!(is_blue(corner), "dead space at {frame}");
                assert!(is_blue(far_side), "dead space at {frame}");
            }
            _ => {
                assert!(is_red(corner), "a full picture hides it at {frame}");
                assert!(is_red(far_side), "a full picture hides it at {frame}");
            }
        }
        frame += 1;
    }
    assert_eq!(frame, 90);
}

#[test]
fn a_full_picture_on_a_coloured_background_still_skips_the_gpu() {
    let mut job = layered_request(
        "layer-background-hidden",
        vec![stretch(
            0.0,
            1.0,
            vec![whole(&large_clip(), 1, 0.0, 1.0, Placement::default())],
        )],
        Vec::new(),
    );
    job.background = [0, 0, 255];

    let report = run_layered(&job);

    assert_eq!(report.gpu, None);
    assert_eq!(report.composited_frames, 0);
}
