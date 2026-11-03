//! Exports whole videos one after another, the way the app would, and says what happened.
//!
//! `cargo run --release -p bareclip-export --example export_files -- <output> <video> <video>...`

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

use bareclip_core::{splice::CutMode, time::Ticks, timeline::TimeRange};
use bareclip_export::export_project;
use bareclip_media::{ExportPart, ExportRequest, probe, scan_frames};

fn part_of(path: &Path) -> ExportPart {
    let cancel = AtomicBool::new(false);
    let info = probe(path).expect("the video can be read");
    let frames = scan_frames(path, &cancel, |_| {}).expect("its frames can be listed");
    ExportPart {
        input: path.to_owned(),
        frames: Arc::new(frames),
        range: TimeRange {
            start: Ticks(0),
            end: info.duration,
        },
        gap_before: Ticks(0),
    }
}

fn main() {
    let mut arguments = std::env::args_os().skip(1).map(PathBuf::from);
    let output = arguments.next().expect("name an output file");
    let parts: Vec<ExportPart> = arguments.map(|path| part_of(&path)).collect();
    let request = ExportRequest {
        output,
        parts,
        mode: CutMode::FrameExact,
    };

    let started = Instant::now();
    let report = export_project(&request, &AtomicBool::new(false), &|_| {}).expect("it exports");
    println!("{report:#?}");
    println!("Took {:.2} s", started.elapsed().as_secs_f64());
}
