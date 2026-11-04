//! Reads pictures and writes a new video from them, for exports that redraw every frame.

use std::path::{Path, PathBuf};

use bareclip_core::{
    picture::{ColorFormat, ColorMatrix, Yuv420},
    time::{Ticks, TimeBase},
};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{
    ChannelLayout, Dictionary, Packet, Rational, codec, color, decoder, encoder, ffi, format,
    format::Pixel,
    frame,
    media::Type,
    software::scaling,
    util::format::sample::{Sample, Type as SampleType},
};

use crate::{
    error::MediaError,
    export::temporary_path,
    media_foundation::OpenEncoder,
    preview::{seek_to_seconds, threaded_context},
    probe::{discard_other_streams, to_time_base},
};

/// Hardware first, because a whole video is being encoded, not just a few frames.
const H264_ENCODERS: [&str; 5] = [
    "h264_nvenc",
    "h264_amf",
    "h264_qsv",
    "h264_mf",
    "libopenh264",
];
pub const SOUND_RATE: u32 = 48_000;
/// Sound is always written as stereo.
pub const SOUND_CHANNELS: usize = 2;
const SOUND_BIT_RATE: usize = 256_000;
const SECONDS_BETWEEN_KEYFRAMES: f64 = 2.0;
const B_FRAMES: i32 = 3;

/// One decoded picture, as it came from the video. Its planes are in the usual 4:2:0 layout,
/// converted if the video stores them some other way.
pub struct DecodedPicture {
    frame: frame::Video,
    time: Ticks,
    color: ColorFormat,
}

impl DecodedPicture {
    /// When this picture appears in its video.
    pub fn time(&self) -> Ticks {
        self.time
    }

    pub fn size(&self) -> (u32, u32) {
        (self.frame.width(), self.frame.height())
    }

    pub fn view(&self) -> Yuv420<'_> {
        Yuv420 {
            width: self.frame.width(),
            height: self.frame.height(),
            y: self.frame.data(0),
            y_stride: self.frame.stride(0),
            u: self.frame.data(1),
            v: self.frame.data(2),
            uv_stride: self.frame.stride(1),
            color: self.color,
        }
    }
}

/// Which colour standard a video uses. Unlabelled video follows the usual convention for its
/// resolution, which is what the preview does too.
pub(crate) fn matrix_of(decoded: &frame::Video) -> ColorMatrix {
    match decoded.color_space() {
        color::Space::BT470BG | color::Space::SMPTE170M => ColorMatrix::Bt601,
        color::Space::BT2020NCL | color::Space::BT2020CL => ColorMatrix::Bt2020,
        color::Space::BT709 => ColorMatrix::Bt709,
        _ if decoded.height() >= 720 => ColorMatrix::Bt709,
        _ => ColorMatrix::Bt601,
    }
}

pub(crate) fn is_full_range(decoded: &frame::Video) -> bool {
    decoded.color_range() == color::Range::JPEG || decoded.format() == Pixel::YUVJ420P
}

pub(crate) fn coefficients_of(matrix: ColorMatrix) -> i32 {
    match matrix {
        ColorMatrix::Bt601 => ffi::SWS_CS_ITU601,
        ColorMatrix::Bt709 => ffi::SWS_CS_ITU709,
        ColorMatrix::Bt2020 => ffi::SWS_CS_BT2020,
    }
}

/// Reads a video's pictures in order, from wherever it is told to start.
pub struct PictureReader {
    input: format::context::Input,
    video_index: usize,
    decoder: decoder::Video,
    core_time_base: TimeBase,
    converter: Option<(scaling::Context, Pixel, u32, u32)>,
    sent_eof: bool,
}

impl PictureReader {
    pub fn open(path: &Path) -> Result<Self, MediaError> {
        crate::init()?;
        let mut input = format::input(path)?;
        let stream = input
            .streams()
            .best(Type::Video)
            .ok_or(MediaError::NoVideoStream)?;
        let video_index = stream.index();
        let time_base = stream.time_base();
        let mut decoder = threaded_context(stream.parameters())?.decoder().video()?;
        decoder.set_packet_time_base(time_base);
        discard_other_streams(&mut input, &[video_index]);
        Ok(Self {
            input,
            video_index,
            decoder,
            core_time_base: to_time_base(time_base),
            converter: None,
            sent_eof: false,
        })
    }

    /// Moves to the keyframe at or before a time. Pictures come from there.
    pub fn seek(&mut self, time: Ticks) -> Result<(), MediaError> {
        seek_to_seconds(&mut self.input, time.as_seconds())?;
        self.decoder.flush();
        self.sent_eof = false;
        Ok(())
    }

    /// The next picture, or nothing at the end of the video.
    pub fn next_picture(&mut self) -> Result<Option<DecodedPicture>, MediaError> {
        let mut decoded = frame::Video::empty();
        loop {
            if self.decoder.receive_frame(&mut decoded).is_ok() {
                return self.finish_picture(decoded).map(Some);
            }
            if self.sent_eof {
                return Ok(None);
            }
            let mut packet = Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() == self.video_index => {
                    // A damaged packet should cost one frame, not the whole export.
                    let _ = self.decoder.send_packet(&packet);
                }
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => {
                    self.decoder.send_eof()?;
                    self.sent_eof = true;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn finish_picture(&mut self, decoded: frame::Video) -> Result<DecodedPicture, MediaError> {
        let time = Ticks::from_pts(decoded.timestamp().unwrap_or_default(), self.core_time_base);
        let matrix = matrix_of(&decoded);
        let full_range = is_full_range(&decoded);
        if matches!(decoded.format(), Pixel::YUV420P | Pixel::YUVJ420P) {
            return Ok(DecodedPicture {
                frame: decoded,
                time,
                color: ColorFormat { matrix, full_range },
            });
        }

        let (pixel, width, height) = (decoded.format(), decoded.width(), decoded.height());
        let stale = self
            .converter
            .as_ref()
            .is_none_or(|(_, from, w, h)| *from != pixel || *w != width || *h != height);
        if stale {
            let mut context = scaling::Context::get(
                pixel,
                width,
                height,
                Pixel::YUV420P,
                width,
                height,
                scaling::Flags::BILINEAR,
            )?;
            let coefficients = coefficients_of(matrix);
            // SAFETY: the context is live, and these calls only read the constant coefficient
            // tables and copy their values into it.
            unsafe {
                let table = ffi::sws_getCoefficients(coefficients);
                ffi::sws_setColorspaceDetails(
                    context.as_mut_ptr(),
                    table,
                    i32::from(full_range),
                    table,
                    0,
                    0,
                    1 << 16,
                    1 << 16,
                );
            }
            self.converter = Some((context, pixel, width, height));
        }
        let mut converted = frame::Video::empty();
        if let Some((context, ..)) = self.converter.as_mut() {
            context.run(&decoded, &mut converted)?;
        }
        Ok(DecodedPicture {
            frame: converted,
            time,
            color: ColorFormat {
                matrix,
                full_range: false,
            },
        })
    }
}

/// The name and language a sound track keeps in the output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoundLabel {
    pub title: Option<String>,
    pub language: Option<String>,
}

/// What a new video will be like.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoSettings {
    pub width: u32,
    pub height: u32,
    /// Frames per second, as a fraction such as 60000 / 1001.
    pub frame_rate: (i32, i32),
    /// For encoders that aim at a bit rate rather than a quality.
    pub bit_rate: usize,
    pub sound_tracks: Vec<SoundLabel>,
    pub quality: Quality,
}

/// How much of a redrawn video's detail to keep, against how large the file gets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quality {
    /// Looks the same as the source, however large that makes the file.
    #[default]
    MatchSource,
    /// About the source's size or smaller, and hard to tell from it.
    Balanced,
    /// About half of that, and it can show on some footage.
    Smaller,
}

impl Quality {
    pub const ALL: [Self; 3] = [Self::MatchSource, Self::Balanced, Self::Smaller];

    /// The encoder's quality number, where lower keeps more.
    fn level(self) -> &'static str {
        match self {
            Self::MatchSource => "19",
            Self::Balanced => "23",
            Self::Smaller => "28",
        }
    }

    /// How much of the source's bit rate to use, on encoders with no quality number.
    pub fn share_of_bit_rate(self) -> f64 {
        match self {
            Self::MatchSource => 1.0,
            Self::Balanced => 0.66,
            Self::Smaller => 0.4,
        }
    }
}

/// With no limit given, the graphics driver quietly applies a limit of its own, which held back
/// footage with a high bit rate when measured. This one only exists to replace it, at one bit for
/// every pixel of every frame, about five times what the busiest recording measured needed. The
/// file's H.264 level follows this number, and players can refuse high levels, so a flat high
/// limit made ordinary 1080p files say they need level 5.1 rather than 4.0.
fn no_real_limit(settings: &VideoSettings) -> u64 {
    let (num, den) = settings.frame_rate;
    let rate = f64::from(num) / f64::from(den.max(1));
    let pixels = f64::from(settings.width) * f64::from(settings.height);
    ((pixels * rate) as u64).clamp(10_000_000, 300_000_000)
}

/// Settings for a whole video at a chosen quality. Unlike the short sections of the fast route,
/// these care about file size too, unless the choice is to match the source.
fn quality_options(name: &str, settings: &VideoSettings) -> Dictionary<'static> {
    let mut options = Dictionary::new();
    let quality = settings.quality;
    let level = quality.level();
    let chosen: Vec<(&str, &str)> = if name.ends_with("_nvenc") {
        vec![
            ("preset", "p5"),
            ("tune", "hq"),
            ("rc", "vbr"),
            ("cq", level),
            ("spatial-aq", "1"),
        ]
    } else if name.ends_with("_amf") {
        vec![
            ("usage", "transcoding"),
            ("rc", "cqp"),
            ("qp_i", level),
            ("qp_p", level),
        ]
    } else if name.ends_with("_qsv") {
        vec![("global_quality", level)]
    } else {
        Vec::new()
    };
    for (key, value) in chosen {
        options.set(key, value);
    }
    if quality == Quality::MatchSource && name.ends_with("_nvenc") {
        let limit = no_real_limit(settings);
        options.set("maxrate", &limit.to_string());
        options.set("bufsize", &(limit * 2).to_string());
    }
    options
}

fn open_video_encoder(
    name: &str,
    settings: &VideoSettings,
) -> Result<(OpenEncoder, Pixel), ffmpeg::Error> {
    let found = encoder::find_by_name(name).ok_or(ffmpeg::Error::EncoderNotFound)?;
    let offered: Vec<Pixel> = found
        .video()?
        .formats()
        .map(Iterator::collect)
        .unwrap_or_default();
    let pixel = if offered.is_empty() || offered.contains(&Pixel::YUV420P) {
        Pixel::YUV420P
    } else {
        Pixel::NV12
    };
    let (rate_num, rate_den) = settings.frame_rate;
    let mut video = codec::context::Context::new_with_codec(found)
        .encoder()
        .video()?;
    video.set_width(settings.width);
    video.set_height(settings.height);
    video.set_format(pixel);
    video.set_time_base(Rational(rate_den, rate_num));
    video.set_frame_rate(Some(Rational(rate_num, rate_den)));
    video.set_colorspace(color::Space::BT709);
    video.set_color_range(color::Range::MPEG);
    // SAFETY: the context is live, and these are plain fields read when the encoder opens.
    // The safe wrappers do not offer them.
    unsafe {
        (*video.as_mut_ptr()).color_primaries = color::Primaries::BT709.into();
        (*video.as_mut_ptr()).color_trc = color::TransferCharacteristic::BT709.into();
    }
    let seconds = f64::from(rate_num) / f64::from(rate_den) * SECONDS_BETWEEN_KEYFRAMES;
    video.set_gop(seconds.round() as u32);
    video.set_max_b_frames(if name.ends_with("_nvenc") {
        B_FRAMES as usize
    } else {
        0
    });
    // MP4 keeps the decoder's settings in its header rather than in the stream.
    video.set_flags(codec::Flags::GLOBAL_HEADER);
    let options = quality_options(name, settings);
    if options.iter().next().is_none() {
        video.set_bit_rate(settings.bit_rate);
    }
    let opened = OpenEncoder::open(video, name, options)?;
    Ok((opened, pixel))
}

fn open_sound_encoder() -> Result<encoder::Audio, ffmpeg::Error> {
    let found = encoder::find_by_name("aac").ok_or(ffmpeg::Error::EncoderNotFound)?;
    let mut audio = codec::context::Context::new_with_codec(found)
        .encoder()
        .audio()?;
    audio.set_rate(SOUND_RATE as i32);
    audio.set_channel_layout(ChannelLayout::STEREO);
    audio.set_format(Sample::F32(SampleType::Planar));
    audio.set_time_base(Rational(1, SOUND_RATE as i32));
    audio.set_bit_rate(SOUND_BIT_RATE);
    audio.set_flags(codec::Flags::GLOBAL_HEADER);
    audio.open()
}

/// One stereo sound track being encoded into a file.
pub(crate) struct SoundOutput {
    pub(crate) encoder: encoder::Audio,
    stream: usize,
    /// Stereo samples waiting to make up a whole block for the encoder.
    pub(crate) waiting: Vec<f32>,
    written_frames: i64,
    pub(crate) block_frames: usize,
}

impl SoundOutput {
    /// Adds a track to a file that hasn't started yet, as its stream number `stream`.
    pub(crate) fn add_to(
        output: &mut format::context::Output,
        stream: usize,
        label: &SoundLabel,
    ) -> Result<Self, MediaError> {
        let audio = open_sound_encoder()?;
        let block_frames = match audio.frame_size() {
            0 => 1024,
            size => size as usize,
        };
        let mut added = output.add_stream(encoder::find(codec::Id::AAC))?;
        added.set_parameters(&audio);
        added.set_time_base(Rational(1, SOUND_RATE as i32));
        let mut metadata = Dictionary::new();
        if let Some(title) = &label.title {
            metadata.set("title", title);
            // This is where MP4 actually keeps a track's name.
            metadata.set("handler_name", title);
        }
        if let Some(language) = &label.language {
            metadata.set("language", language);
        }
        added.set_metadata(metadata);
        Ok(Self {
            encoder: audio,
            stream,
            waiting: Vec::new(),
            written_frames: 0,
            block_frames,
        })
    }

    /// Sends this many frames of what is waiting to the encoder.
    pub(crate) fn send_block(&mut self, frames: usize) -> Result<(), MediaError> {
        let mut block = frame::Audio::new(
            Sample::F32(SampleType::Planar),
            frames,
            ChannelLayout::STEREO,
        );
        block.set_rate(SOUND_RATE);
        block.set_pts(Some(self.written_frames));
        for channel in 0..SOUND_CHANNELS {
            let plane = block.plane_mut::<f32>(channel);
            for (into, sample) in plane
                .iter_mut()
                .zip(self.waiting.iter().skip(channel).step_by(SOUND_CHANNELS))
            {
                *into = *sample;
            }
        }
        self.waiting.drain(..frames * SOUND_CHANNELS);
        self.written_frames += frames as i64;
        self.encoder.send_frame(&block)?;
        Ok(())
    }

    /// Writes whatever the encoder has finished.
    pub(crate) fn drain(
        &mut self,
        time_base: Rational,
        output: &mut format::context::Output,
    ) -> Result<(), MediaError> {
        let from = Rational(1, SOUND_RATE as i32);
        let mut packet = Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.stream);
            packet.rescale_ts(from, time_base);
            packet.write_interleaved(output)?;
        }
        Ok(())
    }
}

/// Copies a plane's rows into a frame with its own row length.
fn copy_plane(
    from: &[u8],
    from_stride: usize,
    into: &mut [u8],
    into_stride: usize,
    rows: usize,
    width: usize,
) {
    for row in 0..rows {
        let source = &from[row * from_stride..][..width];
        into[row * into_stride..][..width].copy_from_slice(source);
    }
}

/// Writes a new H.264 video in MP4, picture by picture and sound by sound. Nothing appears at
/// the real path until it is finished, and giving up leaves nothing behind.
pub struct VideoWriter {
    output: Option<format::context::Output>,
    temporary: PathBuf,
    target: PathBuf,
    encoder: OpenEncoder,
    encoder_name: String,
    pixel: Pixel,
    /// The picture last sent, kept so that showing it again costs almost nothing.
    picture: frame::Video,
    next_frame: i64,
    video_time_base: Rational,
    stream_time_base: Rational,
    sound: Vec<SoundOutput>,
    sound_time_bases: Vec<Rational>,
}

impl VideoWriter {
    pub fn create(path: &Path, settings: &VideoSettings) -> Result<Self, MediaError> {
        Self::create_with_any_of(path, settings, &H264_ENCODERS)
    }

    /// Writes with the first of these encoders that works here.
    pub fn create_with_any_of(
        path: &Path,
        settings: &VideoSettings,
        encoders: &[&str],
    ) -> Result<Self, MediaError> {
        crate::init()?;
        let (encoder, encoder_name, pixel) = encoders
            .iter()
            .find_map(|name| {
                open_video_encoder(name, settings)
                    .ok()
                    .map(|(encoder, pixel)| (encoder, (*name).to_owned(), pixel))
            })
            .ok_or(ffmpeg::Error::EncoderNotFound)?;

        let temporary = temporary_path(path);
        let mut output = format::output(&temporary)?;
        let video_time_base = Rational(settings.frame_rate.1, settings.frame_rate.0);
        {
            let mut stream = output.add_stream(encoder::find(codec::Id::H264))?;
            stream.set_parameters(&*encoder);
            stream.set_time_base(video_time_base);
            stream.set_avg_frame_rate(Rational(settings.frame_rate.0, settings.frame_rate.1));
        }
        let mut sound = Vec::new();
        for (index, label) in settings.sound_tracks.iter().enumerate() {
            sound.push(SoundOutput::add_to(&mut output, index + 1, label)?);
        }
        output.write_header()?;

        let stream_time_base = output
            .stream(0)
            .ok_or(MediaError::NoVideoStream)?
            .time_base();
        let sound_time_bases = sound
            .iter()
            .map(|track| {
                output
                    .stream(track.stream)
                    .map_or(Rational(1, SOUND_RATE as i32), |stream| stream.time_base())
            })
            .collect();
        Ok(Self {
            output: Some(output),
            temporary,
            target: path.to_owned(),
            picture: frame::Video::new(pixel, settings.width, settings.height),
            encoder,
            encoder_name,
            pixel,
            next_frame: 0,
            video_time_base,
            stream_time_base,
            sound,
            sound_time_bases,
        })
    }

    pub fn encoder_name(&self) -> &str {
        &self.encoder_name
    }

    fn output(&mut self) -> Result<&mut format::context::Output, MediaError> {
        self.output
            .as_mut()
            .ok_or(MediaError::Ffmpeg(ffmpeg::Error::InvalidData))
    }

    fn drain_video(&mut self) -> Result<(), MediaError> {
        let (from, to) = (self.video_time_base, self.stream_time_base);
        let mut packet = Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(0);
            packet.rescale_ts(from, to);
            let output = self
                .output
                .as_mut()
                .ok_or(MediaError::Ffmpeg(ffmpeg::Error::InvalidData))?;
            packet.write_interleaved(output)?;
        }
        Ok(())
    }

    fn send_picture(&mut self) -> Result<(), MediaError> {
        self.picture.set_pts(Some(self.next_frame));
        self.next_frame += 1;
        self.encoder.send_frame(&self.picture)?;
        self.drain_video()
    }

    /// Adds a picture of the video's exact size, in the colours the output is labelled with.
    pub fn write_picture(&mut self, picture: &Yuv420) -> Result<(), MediaError> {
        // SAFETY: the frame is live. The encoder may still be holding the last picture, and
        // this gives the frame new memory if so, rather than changing what the encoder sees.
        let writable = unsafe { ffi::av_frame_make_writable(self.picture.as_mut_ptr()) };
        if writable < 0 {
            return Err(ffmpeg::Error::from(writable).into());
        }
        let (width, height) = (picture.width as usize, picture.height as usize);
        let (chroma_width, chroma_height) = {
            let size = picture.chroma_size();
            (size.0 as usize, size.1 as usize)
        };
        let stride = self.picture.stride(0);
        copy_plane(
            picture.y,
            picture.y_stride,
            self.picture.data_mut(0),
            stride,
            height,
            width,
        );
        match self.pixel {
            Pixel::NV12 => {
                let stride = self.picture.stride(1);
                let plane = self.picture.data_mut(1);
                for row in 0..chroma_height {
                    let u = &picture.u[row * picture.uv_stride..][..chroma_width];
                    let v = &picture.v[row * picture.uv_stride..][..chroma_width];
                    let into = &mut plane[row * stride..][..chroma_width * 2];
                    for (pair, (u, v)) in
                        into.as_chunks_mut::<2>().0.iter_mut().zip(u.iter().zip(v))
                    {
                        *pair = [*u, *v];
                    }
                }
            }
            _ => {
                let stride = self.picture.stride(1);
                copy_plane(
                    picture.u,
                    picture.uv_stride,
                    self.picture.data_mut(1),
                    stride,
                    chroma_height,
                    chroma_width,
                );
                let stride = self.picture.stride(2);
                copy_plane(
                    picture.v,
                    picture.uv_stride,
                    self.picture.data_mut(2),
                    stride,
                    chroma_height,
                    chroma_width,
                );
            }
        }
        self.send_picture()
    }

    /// Shows the last picture for one more frame.
    pub fn repeat_picture(&mut self) -> Result<(), MediaError> {
        self.send_picture()
    }

    fn encode_sound_block(&mut self, track: usize, frames: usize) -> Result<(), MediaError> {
        self.sound[track].send_block(frames)?;
        self.drain_sound(track)
    }

    fn drain_sound(&mut self, track: usize) -> Result<(), MediaError> {
        let to = self.sound_time_bases[track];
        let Self { sound, output, .. } = self;
        let output = output
            .as_mut()
            .ok_or(MediaError::Ffmpeg(ffmpeg::Error::InvalidData))?;
        sound[track].drain(to, output)
    }

    /// Adds stereo samples to the end of a sound track.
    pub fn write_sound(&mut self, track: usize, samples: &[f32]) -> Result<(), MediaError> {
        self.sound[track].waiting.extend_from_slice(samples);
        let block = self.sound[track].block_frames;
        while self.sound[track].waiting.len() >= block * SOUND_CHANNELS {
            self.encode_sound_block(track, block)?;
        }
        Ok(())
    }

    /// Adds silence to the end of a sound track.
    pub fn write_silence(&mut self, track: usize, frames: usize) -> Result<(), MediaError> {
        const CHUNK_FRAMES: usize = 4096;
        let silence = [0.0; CHUNK_FRAMES * SOUND_CHANNELS];
        let mut left = frames;
        while left > 0 {
            let now = left.min(CHUNK_FRAMES);
            self.write_sound(track, &silence[..now * SOUND_CHANNELS])?;
            left -= now;
        }
        Ok(())
    }

    /// Completes the file and puts it in place.
    pub fn finish(mut self) -> Result<(), MediaError> {
        self.encoder.send_eof()?;
        self.drain_video()?;
        for track in 0..self.sound.len() {
            let left = self.sound[track].waiting.len() / SOUND_CHANNELS;
            if left > 0 {
                self.encode_sound_block(track, left)?;
            }
            self.sound[track].encoder.send_eof()?;
            self.drain_sound(track)?;
        }
        self.output()?.write_trailer()?;
        // The file has to be closed before it can be moved.
        self.output = None;
        std::fs::rename(&self.temporary, &self.target)?;
        Ok(())
    }
}

impl Drop for VideoWriter {
    fn drop(&mut self) {
        if self.output.take().is_some() {
            let _ = std::fs::remove_file(&self.temporary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_is_tried_before_software_for_whole_videos() {
        assert_eq!(H264_ENCODERS[0], "h264_nvenc");
        assert_eq!(H264_ENCODERS[H264_ENCODERS.len() - 1], "libopenh264");
    }

    #[test]
    fn rows_are_copied_without_their_padding() {
        let from = [1, 2, 9, 3, 4, 9];
        let mut into = [0; 8];

        copy_plane(&from, 3, &mut into, 4, 2, 2);

        assert_eq!(into, [1, 2, 0, 0, 3, 4, 0, 0]);
    }

    #[test]
    fn encoders_with_a_quality_setting_get_one() {
        crate::init().expect("FFmpeg starts");

        assert!(
            quality_options(
                "h264_nvenc",
                &settings_at((1920, 1080), 30, Quality::default())
            )
            .iter()
            .next()
            .is_some()
        );
        assert!(
            quality_options(
                "h264_mf",
                &settings_at((1920, 1080), 30, Quality::default())
            )
            .iter()
            .next()
            .is_none()
        );
    }

    fn settings_at(size: (u32, u32), rate: i32, quality: Quality) -> VideoSettings {
        VideoSettings {
            width: size.0,
            height: size.1,
            frame_rate: (rate, 1),
            bit_rate: 1_000_000,
            sound_tracks: Vec::new(),
            quality,
        }
    }

    #[test]
    fn each_quality_asks_the_encoder_for_its_own_level() {
        let cq = |quality| {
            quality_options("h264_nvenc", &settings_at((1920, 1080), 30, quality))
                .get("cq")
                .map(str::to_owned)
        };
        assert_eq!(cq(Quality::MatchSource).as_deref(), Some("19"));
        assert_eq!(cq(Quality::Balanced).as_deref(), Some("23"));
        assert_eq!(cq(Quality::Smaller).as_deref(), Some("28"));
    }

    #[test]
    fn only_matching_the_source_lifts_the_drivers_hidden_limit() {
        let limit = |quality| {
            quality_options("h264_nvenc", &settings_at((1920, 1080), 30, quality))
                .get("maxrate")
                .map(str::to_owned)
        };
        assert_eq!(limit(Quality::MatchSource).as_deref(), Some("62208000"));
        assert_eq!(limit(Quality::Balanced), None);
        assert_eq!(limit(Quality::Smaller), None);
    }

    #[test]
    fn the_limit_grows_with_the_picture_but_stays_in_sensible_bounds() {
        let limit = |size, rate| no_real_limit(&settings_at(size, rate, Quality::MatchSource));
        assert_eq!(limit((1920, 1080), 60), 124_416_000);
        assert_eq!(limit((320, 180), 30), 10_000_000);
        assert_eq!(limit((3840, 2160), 120), 300_000_000);
    }
}
