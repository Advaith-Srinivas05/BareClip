//! Exports the start of a video as a GIF or AV1, the way the app would, and says how it went.
//!
//! `cargo run --release -p bareclip-export --example export_as -- <output> <video> <gif|av1|h264> [seconds] [shorter side] [fps]`
//!
//! Set `BARECLIP_GPU` to a chip's vendor and device numbers, such as `1002:1638`, to run on that
//! chip the way picking it in Settings does.

use std::{path::PathBuf, sync::atomic::AtomicBool, time::Instant};

use bareclip_core::{placement::Placement, speed::Speed, time::Ticks, timeline::TimeRange};
use bareclip_export::{
    ExportRate, LayeredClip, LayeredRequest, LayeredStretch, SoundPlan, export_layered_as,
    gif_size_for,
};
use bareclip_media::{ChipMaker, Quality, VideoFormat, probe};
use bareclip_render::GpuId;

fn main() {
    let chip = std::env::var("BARECLIP_GPU")
        .ok()
        .and_then(|text| GpuId::parse(&text));
    bareclip_render::choose_gpu(chip);
    bareclip_media::follow_chip(chip.and_then(|chip| ChipMaker::from_vendor(chip.vendor)));
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let output = PathBuf::from(arguments.first().expect("name an output"));
    let input = PathBuf::from(arguments.get(1).expect("name a video"));
    let format = match arguments.get(2).map(String::as_str) {
        Some("gif") => VideoFormat::Gif,
        Some("av1") => VideoFormat::Av1,
        Some("h264") => VideoFormat::H264,
        _ => panic!("say gif, av1 or h264"),
    };
    let number = |place: usize| {
        arguments
            .get(place)
            .and_then(|text| text.parse::<f64>().ok())
    };
    let info = probe(&input).expect("the video can be read");
    let seconds = number(3).unwrap_or(info.duration.as_seconds());
    // A video keeps its own size and rate unless told otherwise, and a GIF starts on its usual ones.
    let is_gif = format == VideoFormat::Gif;
    let shorter = number(4).unwrap_or(if is_gif { 480.0 } else { 1_000_000.0 }) as u32;
    let fps = number(5).or(is_gif.then_some(15.0));
    let source = TimeRange {
        start: Ticks(0),
        end: Ticks::from_seconds(seconds).min(info.duration),
    };
    let own = (info.video.width, info.video.height);
    let canvas = gif_size_for(own, shorter);
    let request = LayeredRequest {
        output: output.clone(),
        stretches: vec![LayeredStretch {
            start: Ticks(0),
            end: source.end,
            stack: vec![LayeredClip {
                key: 1,
                input,
                start: Ticks(0),
                source,
                placement: Placement::default(),
                speed: Speed::default(),
                title: None,
                look: Default::default(),
                join_in: None,
                join_out: None,
            }],
        }],
        sound: SoundPlan::default(),
        canvas: Some(canvas),
        background: [0, 0, 0],
        quality: Quality::MatchSource,
        rate: fps.map_or(ExportRate::MatchSource, |fps| ExportRate::Fps(fps as u32)),
    };
    let started = Instant::now();
    let report = export_layered_as(&request, format, &AtomicBool::new(false), &|_| {})
        .expect("the export succeeds");
    println!(
        "{:.2} MB in {:.2} s, {} frames at {} x {}, {} on {:?}",
        report.file_bytes.unwrap_or(0) as f64 / 1_048_576.0,
        started.elapsed().as_secs_f64(),
        report.frames,
        report.width,
        report.height,
        report.encoder,
        report.gpu,
    );
}
