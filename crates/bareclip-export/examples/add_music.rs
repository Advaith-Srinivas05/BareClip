//! Copies a whole video and mixes a song under its sound, to measure how long that takes.
//! `cargo run --release -p bareclip-export --example add_music -- <output> <video> <song>`

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use bareclip_audio::{Bus, MixSpan};
use bareclip_core::{sound::ClipSound, splice::CutMode, time::Ticks, timeline::TimeRange};
use bareclip_export::{SoundPlan, export_with_new_sound};
use bareclip_media::{ExportPart, ExportRequest, SoundLabel, probe, scan_frames};

fn main() {
    let mut arguments = std::env::args_os().skip(1).map(PathBuf::from);
    let (Some(output), Some(video), Some(song)) =
        (arguments.next(), arguments.next(), arguments.next())
    else {
        eprintln!("Usage: add_music <output> <video> <song>");
        return;
    };
    bareclip_media::init().expect("FFmpeg starts");
    let cancel = AtomicBool::new(false);
    let info = probe(&video).expect("the video can be read");
    let whole = TimeRange {
        start: Ticks(0),
        end: info.duration,
    };
    let span = |path: &PathBuf, volume| MixSpan {
        path: path.clone(),
        start: Ticks(0),
        source: whole,
        sound: ClipSound {
            volume,
            ..ClipSound::default()
        },
        speed: Default::default(),
        bus: Bus::All(0),
    };
    let request = ExportRequest {
        output,
        parts: vec![ExportPart {
            input: video.clone(),
            frames: Arc::new(scan_frames(&video, &cancel, |_| {}).expect("the frames can be read")),
            range: whole,
            gap_before: Ticks(0),
        }],
        mode: CutMode::FrameExact,
    };
    let plan = SoundPlan {
        spans: vec![span(&video, 1_000_000), span(&song, 400_000)],
        tracks: vec![SoundLabel::default()],
    };
    let report =
        export_with_new_sound(&request, &plan, &cancel, &|_| {}).expect("the export succeeds");
    println!("{report:#?}");
}
