//! The only crate that calls FFmpeg.

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
