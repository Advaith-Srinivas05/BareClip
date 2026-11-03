//! Measures the speed targets on a real file.
//! Run with: ./scripts/cargo.ps1 run --release -p bareclip-media --example bench '--' <file>

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

use bareclip_core::{
    splice::{CutMode, FrameIndex},
    time::Ticks,
    timeline::TimeRange,
};
use bareclip_media::{
    DecodeStep, ExportPart, ExportRequest, VideoPreview, export, probe, scan_frames,
};

fn range(start: f64, end: f64) -> TimeRange {
    TimeRange {
        start: Ticks::from_seconds(start),
        end: Ticks::from_seconds(end),
    }
}

fn run_export(
    input: &Path,
    frames: &Arc<FrameIndex>,
    label: &str,
    ranges: Vec<TimeRange>,
    mode: CutMode,
) {
    let extension = input
        .extension()
        .map_or_else(|| "mp4".into(), |e| e.to_string_lossy().into_owned());
    // Never beside the video, so measuring someone's recordings leaves their folder as it was.
    let output = std::env::temp_dir().join(format!(
        "bareclip-bench-{}-{label}.{extension}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&output);
    let request = ExportRequest {
        output: output.clone(),
        parts: ranges
            .into_iter()
            .map(|range| ExportPart {
                input: input.to_owned(),
                frames: frames.clone(),
                range,
                gap_before: Ticks(0),
            })
            .collect(),
        mode,
    };
    let report = export(&request, &AtomicBool::new(false), &|_| {}).expect("the export works");
    let size = std::fs::metadata(&output).map_or(0, |meta| meta.len());
    println!(
        "export {label:<22} {:>7.2} s  copied {:>6}  encoded {:>5}  encoder {:<12} {:>7.1} MB  {}",
        report.elapsed.as_secs_f64(),
        report.copied_frames,
        report.encoded_frames,
        report.encoder.as_deref().unwrap_or("none"),
        size as f64 / 1_048_576.0,
        output.display(),
    );
    let _ = std::fs::remove_file(&output);
}

/// What it costs to read the pictures and sound shape clips show on the timeline.
fn bench_strip(input: &std::path::Path, duration: bareclip_core::time::Ticks) {
    let spacing = bareclip_media::picture_spacing(
        duration,
        bareclip_core::time::Ticks::from_seconds(2.0),
        240,
    );
    let started = Instant::now();
    let (mut count, mut bytes) = (0, 0);
    bareclip_media::read_strip_pictures(
        input,
        duration,
        spacing,
        (128, 72),
        &AtomicBool::new(false),
        |picture| {
            count += 1;
            bytes += picture.rgba.len();
        },
    )
    .expect("the pictures can be read");
    println!(
        "strip pictures: {count} in {:.2} s, {:.1} MB, spaced {:.1} s",
        started.elapsed().as_secs_f64(),
        bytes as f64 / 1_048_576.0,
        spacing.as_seconds(),
    );
    let started = Instant::now();
    let shape = bareclip_media::read_sound_shape(input, &AtomicBool::new(false), |_| {})
        .expect("the sound can be read")
        .expect("it was not cancelled");
    println!(
        "sound shape: {} peaks in {:.2} s, {:.2} MB",
        shape.peaks.len(),
        started.elapsed().as_secs_f64(),
        shape.peaks.len() as f64 / 1_048_576.0,
    );
}

fn main() {
    let input = PathBuf::from(std::env::args().nth(1).expect("pass a video file"));
    if std::env::args().nth(2).as_deref() == Some("strip") {
        let info = probe(&input).expect("the file can be read");
        bench_strip(&input, info.duration);
        return;
    }

    let started = Instant::now();
    let info = probe(&input).expect("the file can be read");
    println!(
        "first open (includes starting FFmpeg) {:>7.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let started = Instant::now();
    probe(&input).expect("the file can be read");
    println!(
        "second open                           {:>7.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    println!(
        "{} {}x{} {:.2} fps, {:.1} s, {:.1} MB, {} audio tracks",
        info.video.codec,
        info.video.width,
        info.video.height,
        info.video.frame_rate,
        info.duration.as_seconds(),
        info.size_bytes as f64 / 1_048_576.0,
        info.audio.len(),
    );

    let started = Instant::now();
    let frames = Arc::new(
        scan_frames(&input, &AtomicBool::new(false), |_| {}).expect("the frames can be listed"),
    );
    println!(
        "reading the frame table                {:>6.2} s  ({} frames, {} clean points)",
        started.elapsed().as_secs_f64(),
        frames.frames().len(),
        frames.clean_points().len(),
    );

    let total = info.duration.as_seconds();
    let middle = (total * 0.2 + 0.37, total * 0.4 + 0.71);
    run_export(
        &input,
        &frames,
        "keyframe",
        vec![range(middle.0, middle.1)],
        CutMode::Keyframe,
    );
    run_export(
        &input,
        &frames,
        "frame-exact",
        vec![range(middle.0, middle.1)],
        CutMode::FrameExact,
    );
    run_export(
        &input,
        &frames,
        "frame-exact-3-sections",
        vec![
            range(total * 0.05 + 0.13, total * 0.1 + 0.29),
            range(total * 0.3 + 0.41, total * 0.35 + 0.53),
            range(total * 0.7 + 0.61, total * 0.8 + 0.17),
        ],
        CutMode::FrameExact,
    );

    let mut preview = VideoPreview::open(&input, 1280, 720).expect("the preview opens");
    let time_base = info.video.time_base;
    let seek_pts = Ticks::from_seconds(total * 0.5).to_pts(time_base);
    preview.seek(seek_pts).expect("seeking works");
    let started = Instant::now();
    let mut shown = 0;
    while started.elapsed().as_secs_f64() < 5.0 {
        match preview.next_step(None).expect("decoding works") {
            DecodeStep::Frame(_) => shown += 1,
            DecodeStep::Skipped { .. } => {}
            DecodeStep::End => break,
        }
    }
    println!(
        "preview decode and convert to 1280x720 {:>6.1} fps",
        f64::from(shown) / started.elapsed().as_secs_f64()
    );

    let mut clean = frames.clean_points();
    clean.sort_unstable();
    let mut seek_times = Vec::new();
    for fraction in [0.11, 0.37, 0.59, 0.83, 0.23, 0.71] {
        // The worst place to land is the last frame before a keyframe, because
        // everything since the previous keyframe has to be decoded to reach it.
        let target = Ticks::from_seconds(total * fraction).to_pts(time_base);
        let next_clean = clean
            .iter()
            .copied()
            .find(|pts| *pts > target)
            .unwrap_or(target);
        let worst = frames.previous_frame_before(next_clean).unwrap_or(target);
        let started = Instant::now();
        preview.seek(worst).expect("seeking works");
        let first = loop {
            match preview.next_step(None).expect("decoding works") {
                DecodeStep::Frame(frame) => break frame,
                DecodeStep::Skipped { .. } => {}
                DecodeStep::End => panic!("the video ended early"),
            }
        };
        let to_first = started.elapsed().as_secs_f64() * 1000.0;
        if first.pts < worst {
            loop {
                match preview.next_step(Some(worst)).expect("decoding works") {
                    DecodeStep::Frame(_) => break,
                    DecodeStep::Skipped { .. } => {}
                    DecodeStep::End => panic!("the video ended early"),
                }
            }
        }
        seek_times.push((to_first, started.elapsed().as_secs_f64() * 1000.0));
    }
    let worst_first = seek_times.iter().map(|time| time.0).fold(0.0, f64::max);
    let worst_exact = seek_times.iter().map(|time| time.1).fold(0.0, f64::max);
    println!(
        "worst-case seek: first picture {worst_first:>6.0} ms, exact frame {worst_exact:>6.0} ms"
    );
}
