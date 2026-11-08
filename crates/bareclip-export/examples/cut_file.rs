//! Keeps pieces of one video with frame-exact cuts, so the parts around each cut are re-encoded
//! and joined to copied video, and says which encoder did it.
//!
//! `cargo run --release -p bareclip-export --example cut_file -- <output> <video> <start-end>...`
//! with times in seconds, such as `1.37-4.2 6.05-8`. Setting `BARECLIP_SKIP_ENCODERS` shows how a
//! machine without those encoders would do it.

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

use bareclip_core::{splice::CutMode, time::Ticks, timeline::TimeRange};
use bareclip_export::export_project;
use bareclip_media::{ExportPart, ExportRequest, scan_frames};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let output = PathBuf::from(arguments.next().expect("name an output file"));
    let input = PathBuf::from(arguments.next().expect("name a video"));
    let frames = Arc::new(
        scan_frames(&input, &AtomicBool::new(false), |_| {}).expect("its frames can be listed"),
    );
    let parts = arguments
        .map(|piece| {
            let (start, end) = piece.split_once('-').expect("pieces look like 1.5-4");
            let seconds = |text: &str| text.parse::<f64>().expect("times are numbers");
            ExportPart {
                input: input.clone(),
                frames: frames.clone(),
                range: TimeRange {
                    start: Ticks::from_seconds(seconds(start)),
                    end: Ticks::from_seconds(seconds(end)),
                },
                gap_before: Ticks(0),
            }
        })
        .collect();
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
