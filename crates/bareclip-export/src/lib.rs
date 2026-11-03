//! Decides how a project gets exported, and does the export that redraws every frame.

mod full;
mod layered;
mod plan;
mod sink;
mod sound;

use std::{path::Path, sync::atomic::AtomicBool, time::Duration};

use bareclip_media::{ExportReport, ExportRequest, MediaError, MediaInfo, export, probe};

pub use layered::{LayeredClip, LayeredRequest, LayeredStretch, export_layered};
pub use plan::{FullPlan, LAYERED_REASON, Route, final_size, frame_rate_for, route_for};
pub use sink::VideoSink;

/// What an export that redrew every frame did.
#[derive(Clone, Debug, PartialEq)]
pub struct FullReport {
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub frames: u64,
    /// Frames drawn by the GPU, because they needed fitting to the canvas.
    pub composited_frames: u64,
    /// Frames that went straight to the encoder.
    pub direct_frames: u64,
    /// Frames that only showed the previous picture again.
    pub repeated_frames: u64,
    pub black_frames: u64,
    pub encoder: String,
    /// The graphics chip that drew frames, if any frame needed one.
    pub gpu: Option<String>,
    pub audio_tracks: usize,
    pub elapsed: Duration,
}

/// What happened in an export, by the route it took.
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectReport {
    Fast(ExportReport),
    Full(FullReport),
}

/// Reads each different video once, in the order they are first used.
fn videos_of(request: &ExportRequest) -> Result<Vec<MediaInfo>, MediaError> {
    let mut videos: Vec<MediaInfo> = Vec::new();
    for part in &request.parts {
        if !videos.iter().any(|info| info.path == part.input) {
            videos.push(probe(&part.input)?);
        }
    }
    Ok(videos)
}

/// Exports the parts by whichever route the project needs.
pub fn export_project(
    request: &ExportRequest,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<ProjectReport, MediaError> {
    bareclip_media::init()?;
    let videos = videos_of(request)?;
    let has_gaps = request.parts.iter().any(|part| part.gap_before.0 > 0);
    match route_for(&videos, has_gaps) {
        Route::Fast => export(request, cancel, progress).map(ProjectReport::Fast),
        Route::Full(_) => {
            full::export_full(request, &FullPlan::for_videos(&videos), cancel, progress)
                .map(ProjectReport::Full)
        }
    }
}

/// Where a full export should go by default, given where a fast one would.
pub fn full_output_path(fast: &Path) -> std::path::PathBuf {
    fast.with_extension("mp4")
}
