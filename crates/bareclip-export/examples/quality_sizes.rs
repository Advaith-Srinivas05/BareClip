//! Redraws a whole video at each quality, the way the app would, and says how large each came out.
//!
//! `cargo run --release -p bareclip-export --example quality_sizes -- <folder> <video> [speed]`
//! The files stay in the folder, so they can be compared with the source.

use std::{path::PathBuf, sync::atomic::AtomicBool, time::Instant};

use bareclip_audio::{Bus, MixSpan};
use bareclip_core::{
    placement::Placement, sound::ClipSound, speed::Speed, time::Ticks, timeline::TimeRange,
};
use bareclip_export::{
    ExportRate, LayeredClip, LayeredRequest, LayeredStretch, SoundPlan, export_layered,
};
use bareclip_media::{Quality, SoundLabel, probe};

fn main() {
    let mut arguments = std::env::args_os().skip(1).map(PathBuf::from);
    let folder = arguments.next().expect("name a folder");
    let input = arguments.next().expect("name a video");
    let speed = arguments.next().map_or_else(Speed::default, |factor| {
        let factor: f64 = factor
            .to_string_lossy()
            .parse()
            .expect("the speed is a number");
        Speed::default().with_factor(factor)
    });
    let info = probe(&input).expect("the video can be read");
    let source = TimeRange {
        start: Ticks(0),
        end: info.duration,
    };
    let source_size = std::fs::metadata(&input).expect("it exists").len();
    println!(
        "source: {:.1} MB, {:.1} s",
        source_size as f64 / 1_048_576.0,
        info.duration.as_seconds()
    );

    for (name, quality) in [
        ("match", Quality::MatchSource),
        ("balanced", Quality::Balanced),
        ("smaller", Quality::Smaller),
    ] {
        let output = folder.join(format!("quality-{name}.mp4"));
        let request = LayeredRequest {
            output: output.clone(),
            stretches: vec![LayeredStretch {
                start: Ticks(0),
                end: speed.on_timeline(info.duration),
                stack: vec![LayeredClip {
                    key: 1,
                    input: input.clone(),
                    start: Ticks(0),
                    source,
                    placement: Placement::default(),
                    speed,
                }],
            }],
            sound: if info.audio.is_empty() {
                SoundPlan::default()
            } else {
                SoundPlan {
                    spans: vec![MixSpan {
                        path: input.clone(),
                        start: Ticks(0),
                        source,
                        sound: ClipSound::default(),
                        speed,
                        bus: Bus::All(0),
                    }],
                    tracks: vec![SoundLabel::default()],
                }
            },
            canvas: Some((info.video.width, info.video.height)),
            background: [0, 0, 0],
            quality,
            rate: ExportRate::MatchSource,
        };
        let started = Instant::now();
        let report = export_layered(&request, &AtomicBool::new(false), &|_| {})
            .expect("the export succeeds");
        let size = std::fs::metadata(&output).expect("it was written").len();
        println!(
            "{name}: {:.1} MB in {:.2} s, {} frames at {} x {}, {}",
            size as f64 / 1_048_576.0,
            started.elapsed().as_secs_f64(),
            report.frames,
            report.width,
            report.height,
            report.encoder,
        );
    }
}
