//! Encodes on its own thread, so that drawing the next frame never waits for the last one to
//! finish encoding. This alone makes a stacked export much faster.

use std::{
    path::Path,
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
};

use bareclip_core::picture::{ColorFormat, Yuv420};
use bareclip_media::{GifWriter, MediaError, VideoFormat, VideoSettings, VideoWriter};

/// Pictures waiting to be encoded. A few is enough to smooth out uneven frames, and each one
/// takes memory.
const PICTURES_WAITING: usize = 4;

/// A picture's planes with no padding, reused from frame to frame.
#[derive(Default)]
struct PictureCopy {
    width: u32,
    height: u32,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

fn copy_rows(from: &[u8], stride: usize, width: usize, rows: usize, into: &mut Vec<u8>) {
    into.clear();
    into.reserve(width * rows);
    for row in 0..rows {
        into.extend_from_slice(&from[row * stride..][..width]);
    }
}

impl PictureCopy {
    fn fill_from(&mut self, picture: &Yuv420) {
        let (chroma_width, chroma_height) = picture.chroma_size();
        let (width, height) = (picture.width as usize, picture.height as usize);
        let (chroma_width, chroma_height) = (chroma_width as usize, chroma_height as usize);
        self.width = picture.width;
        self.height = picture.height;
        copy_rows(picture.y, picture.y_stride, width, height, &mut self.y);
        copy_rows(
            picture.u,
            picture.uv_stride,
            chroma_width,
            chroma_height,
            &mut self.u,
        );
        copy_rows(
            picture.v,
            picture.uv_stride,
            chroma_width,
            chroma_height,
            &mut self.v,
        );
    }

    fn view(&self) -> Yuv420<'_> {
        Yuv420 {
            width: self.width,
            height: self.height,
            y: &self.y,
            y_stride: self.width as usize,
            u: &self.u,
            v: &self.v,
            uv_stride: self.width.div_ceil(2) as usize,
            color: ColorFormat::OUTPUT,
        }
    }
}

enum Job {
    Picture(PictureCopy),
    Repeat,
    Sound(usize, Vec<f32>),
    Silence(usize, usize),
    Finish,
}

/// Whatever is making the file.
enum Writer {
    Video(Box<VideoWriter>),
    Gif(Box<GifWriter>),
}

impl Writer {
    fn create(path: &Path, settings: &VideoSettings) -> Result<Self, MediaError> {
        Ok(if settings.format == VideoFormat::Gif {
            Self::Gif(Box::new(GifWriter::create(path, settings)?))
        } else {
            Self::Video(Box::new(VideoWriter::create(path, settings)?))
        })
    }

    fn encoder_name(&self) -> String {
        match self {
            Self::Video(writer) => writer.encoder_name().to_owned(),
            Self::Gif(writer) => writer.encoder_name().to_owned(),
        }
    }

    fn write_picture(&mut self, picture: &Yuv420) -> Result<(), MediaError> {
        match self {
            Self::Video(writer) => writer.write_picture(picture),
            Self::Gif(writer) => writer.write_picture(picture),
        }
    }

    fn repeat_picture(&mut self) -> Result<(), MediaError> {
        match self {
            Self::Video(writer) => writer.repeat_picture(),
            Self::Gif(writer) => writer.repeat_picture(),
        }
    }

    /// A GIF has nowhere to keep sound.
    fn write_sound(&mut self, track: usize, samples: &[f32]) -> Result<(), MediaError> {
        match self {
            Self::Video(writer) => writer.write_sound(track, samples),
            Self::Gif(_) => Ok(()),
        }
    }

    fn write_silence(&mut self, track: usize, frames: usize) -> Result<(), MediaError> {
        match self {
            Self::Video(writer) => writer.write_silence(track, frames),
            Self::Gif(_) => Ok(()),
        }
    }

    fn finish(self) -> Result<(), MediaError> {
        match self {
            Self::Video(writer) => writer.finish(),
            Self::Gif(writer) => writer.finish(),
        }
    }
}

fn run_jobs(
    mut writer: Writer,
    jobs: &Receiver<Job>,
    spare: &SyncSender<PictureCopy>,
) -> Result<(), MediaError> {
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Picture(picture) => {
                writer.write_picture(&picture.view())?;
                let _ = spare.try_send(picture);
            }
            Job::Repeat => writer.repeat_picture()?,
            Job::Sound(track, samples) => writer.write_sound(track, &samples)?,
            Job::Silence(track, frames) => writer.write_silence(track, frames)?,
            Job::Finish => return writer.finish(),
        }
    }
    // Everything stopped early. Dropping the writer removes the unfinished file.
    Ok(())
}

/// Hands a new video's pictures and sound to the thread that encodes them.
pub struct VideoSink {
    jobs: Option<SyncSender<Job>>,
    spare: Receiver<PictureCopy>,
    worker: Option<JoinHandle<Result<(), MediaError>>>,
    encoder_name: String,
}

impl VideoSink {
    pub fn create(path: &Path, settings: &VideoSettings) -> Result<Self, MediaError> {
        let writer = Writer::create(path, settings)?;
        let encoder_name = writer.encoder_name();
        let (jobs, received) = mpsc::sync_channel(PICTURES_WAITING);
        let (returned, spare) = mpsc::sync_channel(PICTURES_WAITING + 1);
        let worker = thread::Builder::new()
            .name("bareclip-export-encode".to_owned())
            .spawn(move || run_jobs(writer, &received, &returned))?;
        Ok(Self {
            jobs: Some(jobs),
            spare,
            worker: Some(worker),
            encoder_name,
        })
    }

    pub fn encoder_name(&self) -> &str {
        &self.encoder_name
    }

    /// Stops the worker and finds out why it stopped.
    fn worker_result(&mut self) -> Result<(), MediaError> {
        self.jobs = None;
        match self.worker.take().map(JoinHandle::join) {
            Some(Ok(result)) => result,
            Some(Err(_)) => Err(MediaError::Render(
                "The encoder stopped unexpectedly.".to_owned(),
            )),
            None => Ok(()),
        }
    }

    fn send(&mut self, job: Job) -> Result<(), MediaError> {
        let delivered = self
            .jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(job).is_ok());
        if delivered {
            return Ok(());
        }
        // The worker only stops early if something went wrong, and it can say what.
        self.worker_result()?;
        Err(MediaError::Render(
            "The encoder stopped unexpectedly.".to_owned(),
        ))
    }

    pub fn write_picture(&mut self, picture: &Yuv420) -> Result<(), MediaError> {
        let mut copy = self.spare.try_recv().unwrap_or_default();
        copy.fill_from(picture);
        self.send(Job::Picture(copy))
    }

    pub fn repeat_picture(&mut self) -> Result<(), MediaError> {
        self.send(Job::Repeat)
    }

    pub fn write_sound(&mut self, track: usize, samples: &[f32]) -> Result<(), MediaError> {
        self.send(Job::Sound(track, samples.to_vec()))
    }

    pub fn write_silence(&mut self, track: usize, frames: usize) -> Result<(), MediaError> {
        self.send(Job::Silence(track, frames))
    }

    /// Completes the file and puts it in place.
    pub fn finish(mut self) -> Result<(), MediaError> {
        self.send(Job::Finish)?;
        self.worker_result()
    }
}

impl Drop for VideoSink {
    /// Giving up waits for the worker, so the unfinished file is gone before anyone looks.
    fn drop(&mut self) {
        let _ = self.worker_result();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareclip_core::picture::BlackPicture;

    #[test]
    fn a_copy_drops_the_padding_from_every_row() {
        let y = [1, 2, 9, 9, 3, 4, 9, 9];
        let (u, v) = ([5, 9, 9], [6, 9, 9]);
        let picture = Yuv420 {
            width: 2,
            height: 2,
            y: &y,
            y_stride: 4,
            u: &u,
            v: &v,
            uv_stride: 3,
            color: ColorFormat::OUTPUT,
        };
        let mut copy = PictureCopy::default();

        copy.fill_from(&picture);

        assert_eq!(copy.y, [1, 2, 3, 4]);
        assert_eq!(copy.u, [5]);
        assert_eq!(copy.v, [6]);
        assert_eq!(copy.view().y_stride, 2);
    }

    #[test]
    fn a_copy_can_be_reused_for_a_picture_of_another_size() {
        let mut copy = PictureCopy::default();
        copy.fill_from(&BlackPicture::new(8, 8).view());

        copy.fill_from(&BlackPicture::new(4, 2).view());

        assert_eq!(copy.y.len(), 8);
        assert_eq!(copy.u.len(), 2);
    }

    #[test]
    fn colours_are_labelled_with_what_the_output_uses() {
        let mut copy = PictureCopy::default();
        copy.fill_from(&BlackPicture::new(4, 2).view());

        assert_eq!(copy.view().color, ColorFormat::OUTPUT);
    }
}
