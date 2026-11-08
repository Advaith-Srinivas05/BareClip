//! The only crate that calls FFmpeg.

mod encoders;
mod error;
mod export;
mod full;
mod media_foundation;
mod nal;
mod preview;
mod probe;
mod remux;
mod resample;
mod still;
mod strip;

pub use encoders::SKIP_VARIABLE as SKIP_ENCODERS_VARIABLE;
pub use error::MediaError;
pub use export::{
    ExportPart, ExportReport, ExportRequest, can_write_gaps, export, export_picture_only,
    plan_export, same_file,
};
pub use full::{
    DecodedPicture, PictureReader, Quality, SOUND_CHANNELS, SOUND_RATE, SoundLabel, VideoSettings,
    VideoWriter,
};
pub use preview::{
    AudioChunk, AudioPreview, AudioStep, DecodeStep, PreviewFrame, PreviewPlanes, VideoPreview,
    fit_preview_size,
};
pub use probe::{AudioInfo, MediaInfo, VideoInfo, join_problem, probe, scan_frames};
pub use remux::{SoundFile, join_picture_and_sound};
pub use resample::RateChange;
pub use still::read_still;
pub use strip::{SoundShape, StripPicture, picture_spacing, read_sound_shape, read_strip_pictures};

use ffmpeg_next as ffmpeg;

/// Details of the FFmpeg libraries loaded at runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FfmpegInfo {
    pub license: String,
    pub configuration: String,
}

/// Starts FFmpeg. Safe to call more than once.
pub fn init() -> Result<(), ffmpeg::Error> {
    ffmpeg::init()
}

pub fn ffmpeg_info() -> FfmpegInfo {
    FfmpegInfo {
        license: ffmpeg::codec::license().to_owned(),
        configuration: ffmpeg::codec::configuration().to_owned(),
    }
}

/// Whether FFmpeg was built with a named encoder, such as `h264_nvenc`.
/// This does not check that the current hardware can run it.
pub fn has_encoder(name: &str) -> bool {
    ffmpeg::encoder::find_by_name(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_build_allows_an_mit_app() {
        init().unwrap();
        let info = ffmpeg_info();

        assert!(
            info.license.starts_with("LGPL"),
            "licence is {}",
            info.license
        );
        assert!(!info.configuration.contains("--enable-gpl"));
        assert!(!info.configuration.contains("--enable-nonfree"));
    }

    #[test]
    fn gpl_encoders_are_absent() {
        init().unwrap();

        assert!(!has_encoder("libx264"));
        assert!(!has_encoder("libx265"));
    }

    #[test]
    fn expected_encoders_are_built_in() {
        init().unwrap();

        for name in [
            "h264_nvenc",
            "hevc_nvenc",
            "h264_amf",
            "h264_qsv",
            "h264_mf",
            "aac",
        ] {
            assert!(has_encoder(name), "{name} is missing");
        }
    }
}
