use std::fmt;

use ffmpeg_next as ffmpeg;

#[derive(Debug)]
pub enum MediaError {
    Ffmpeg(ffmpeg::Error),
    Io(std::io::Error),
    NoVideoStream,
    NoFrames,
    NothingToExport,
    SameInputAndOutput,
    Cancelled,
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ffmpeg(error) => write!(f, "FFmpeg reported an error: {error}"),
            Self::Io(error) => write!(f, "The file could not be read or written: {error}"),
            Self::NoVideoStream => f.write_str("This file has no video track."),
            Self::NoFrames => f.write_str("No video frames could be read from this file."),
            Self::NothingToExport => f.write_str("Mark a section to keep before exporting."),
            Self::SameInputAndOutput => {
                f.write_str("Choose a different file name, so the original stays untouched.")
            }
            Self::Cancelled => f.write_str("Export cancelled."),
        }
    }
}

impl std::error::Error for MediaError {}

impl From<ffmpeg::Error> for MediaError {
    fn from(error: ffmpeg::Error) -> Self {
        Self::Ffmpeg(error)
    }
}

impl From<std::io::Error> for MediaError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
