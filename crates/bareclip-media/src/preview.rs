//! Decodes video and audio for on-screen playback.

use std::path::Path;

use bareclip_core::{
    picture::{ColorFormat, Yuv420},
    time::{Ticks, TimeBase},
};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{
    ChannelLayout, Packet, Rational, codec, color, decoder, ffi, format,
    format::Pixel,
    frame,
    media::Type,
    software::{resampling, scaling},
    util::format::sample::{Sample, Type as SampleType},
};

use crate::{
    error::MediaError,
    full::{coefficients_of, is_full_range, matrix_of},
    probe::{discard_other_streams, to_time_base},
};

pub(crate) fn threaded_context(
    parameters: codec::Parameters,
) -> Result<codec::context::Context, ffmpeg::Error> {
    let mut context = codec::context::Context::from_parameters(parameters)?;
    context.set_threading(codec::threading::Config {
        kind: codec::threading::Type::Frame,
        count: 0,
    });
    Ok(context)
}

pub(crate) fn seek_to_seconds(
    input: &mut format::context::Input,
    seconds: f64,
) -> Result<(), ffmpeg::Error> {
    let target = (seconds * f64::from(ffi::AV_TIME_BASE)) as i64;
    if target <= 0 {
        input.seek(0, ..0)
    } else {
        input.seek(target, ..target)
    }
}

/// The largest size that fits within the limits, keeping the shape of the picture.
/// Sizes are even, and video is never scaled up.
pub fn fit_preview_size(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let scale = (f64::from(max_width) / f64::from(width.max(1)))
        .min(f64::from(max_height) / f64::from(height.max(1)))
        .min(1.0);
    let even = |value: f64| ((value.round() as u32) & !1).max(2);
    (
        even(f64::from(width) * scale),
        even(f64::from(height) * scale),
    )
}

/// Sound always comes out as interleaved stereo.
pub(crate) const PREVIEW_CHANNELS: usize = 2;

/// A picture's stored values, tightly packed, for drawing with others on the GPU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewPlanes {
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub color: ColorFormat,
}

/// One decoded picture, ready to draw.
#[derive(Clone, Debug)]
pub struct PreviewFrame {
    pub pts: i64,
    pub time: Ticks,
    pub width: u32,
    pub height: u32,
    /// Empty when the preview was asked for planes instead.
    pub rgba: Vec<u8>,
    pub planes: Option<PreviewPlanes>,
}

impl PreviewFrame {
    /// The picture as the GPU compositor takes it, if it was decoded that way.
    pub fn view(&self) -> Option<Yuv420<'_>> {
        let planes = self.planes.as_ref()?;
        Some(Yuv420 {
            width: self.width,
            height: self.height,
            y: &planes.y,
            y_stride: self.width as usize,
            u: &planes.u,
            v: &planes.v,
            uv_stride: self.width.div_ceil(2) as usize,
            color: planes.color,
        })
    }
}

/// Whether a decoded frame was shown or passed over on the way to a target.
#[derive(Debug)]
pub enum DecodeStep {
    Frame(PreviewFrame),
    Skipped { pts: i64 },
    End,
}

pub struct VideoPreview {
    input: format::context::Input,
    video_index: usize,
    decoder: decoder::Video,
    scaler: Option<(scaling::Context, ffmpeg::format::Pixel, u32, u32)>,
    /// Hands out planes rather than RGBA, for drawing on the GPU.
    planes: bool,
    time_base: Rational,
    core_time_base: TimeBase,
    output_size: (u32, u32),
    sent_eof: bool,
}

impl VideoPreview {
    pub fn open(path: &Path, max_width: u32, max_height: u32) -> Result<Self, MediaError> {
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
        let output_size =
            fit_preview_size(decoder.width(), decoder.height(), max_width, max_height);
        discard_other_streams(&mut input, &[video_index]);

        Ok(Self {
            input,
            video_index,
            decoder,
            scaler: None,
            planes: false,
            time_base,
            core_time_base: to_time_base(time_base),
            output_size,
            sent_eof: false,
        })
    }

    /// Makes pictures come out as planes for the GPU rather than as RGBA. Skipping the colour
    /// conversion is cheaper, and the GPU does it the same way for every layer.
    pub fn with_planes(mut self) -> Self {
        self.planes = true;
        self
    }

    /// Moves to the keyframe at or before a display time.
    pub fn seek(&mut self, pts: i64) -> Result<(), MediaError> {
        let seconds = f64::from(self.time_base) * pts as f64;
        seek_to_seconds(&mut self.input, seconds)?;
        self.decoder.flush();
        self.sent_eof = false;
        Ok(())
    }

    /// Decodes the next frame. Frames before `show_from` are passed over quickly,
    /// because converting them for display would only slow a seek down.
    pub fn next_step(&mut self, show_from: Option<i64>) -> Result<DecodeStep, MediaError> {
        let mut decoded = frame::Video::empty();
        loop {
            if self.decoder.receive_frame(&mut decoded).is_ok() {
                let pts = decoded.timestamp().unwrap_or_default();
                if show_from.is_some_and(|from| pts < from) {
                    return Ok(DecodeStep::Skipped { pts });
                }
                return Ok(DecodeStep::Frame(self.convert(&decoded, pts)?));
            }
            if self.sent_eof {
                return Ok(DecodeStep::End);
            }

            let mut packet = Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() == self.video_index => {
                    // A damaged packet should cost one frame, not the whole preview.
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

    /// Scales a picture into tightly packed planes, keeping its stored values as they are.
    fn convert_to_planes(
        &mut self,
        decoded: &frame::Video,
        pts: i64,
    ) -> Result<PreviewFrame, MediaError> {
        let (out_width, out_height) = self.output_size;
        let matrix = matrix_of(decoded);
        let full_range = is_full_range(decoded);
        let needs_new = self
            .scaler
            .as_ref()
            .is_none_or(|(_, format, width, height)| {
                *format != decoded.format()
                    || *width != decoded.width()
                    || *height != decoded.height()
            });
        if needs_new {
            let mut context = scaling::Context::get(
                decoded.format(),
                decoded.width(),
                decoded.height(),
                Pixel::YUV420P,
                out_width,
                out_height,
                scaling::Flags::BILINEAR,
            )?;
            // SAFETY: the context is live, and these calls only read the constant coefficient
            // tables and copy their values into it.
            unsafe {
                let table = ffi::sws_getCoefficients(coefficients_of(matrix));
                let range = i32::from(full_range);
                ffi::sws_setColorspaceDetails(
                    context.as_mut_ptr(),
                    table,
                    range,
                    table,
                    range,
                    0,
                    1 << 16,
                    1 << 16,
                );
            }
            self.scaler = Some((context, decoded.format(), decoded.width(), decoded.height()));
        }

        let mut converted = frame::Video::empty();
        if let Some((scaler, ..)) = self.scaler.as_mut() {
            scaler.run(decoded, &mut converted)?;
        }

        let packed = |plane: usize, width: usize, height: usize| {
            let stride = converted.stride(plane);
            let data = converted.data(plane);
            let mut values = Vec::with_capacity(width * height);
            for row in 0..height {
                values.extend_from_slice(&data[stride * row..stride * row + width]);
            }
            values
        };
        let (width, height) = (out_width as usize, out_height as usize);
        let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
        Ok(PreviewFrame {
            pts,
            time: Ticks::from_pts(pts, self.core_time_base),
            width: out_width,
            height: out_height,
            rgba: Vec::new(),
            planes: Some(PreviewPlanes {
                y: packed(0, width, height),
                u: packed(1, chroma_width, chroma_height),
                v: packed(2, chroma_width, chroma_height),
                color: ColorFormat { matrix, full_range },
            }),
        })
    }

    fn convert(&mut self, decoded: &frame::Video, pts: i64) -> Result<PreviewFrame, MediaError> {
        if self.planes {
            return self.convert_to_planes(decoded, pts);
        }
        let (out_width, out_height) = self.output_size;
        let needs_new = self
            .scaler
            .as_ref()
            .is_none_or(|(_, format, width, height)| {
                *format != decoded.format()
                    || *width != decoded.width()
                    || *height != decoded.height()
            });
        if needs_new {
            let mut context = scaling::Context::get(
                decoded.format(),
                decoded.width(),
                decoded.height(),
                ffmpeg::format::Pixel::RGBA,
                out_width,
                out_height,
                scaling::Flags::BILINEAR,
            )?;
            set_color_conversion(&mut context, decoded);
            self.scaler = Some((context, decoded.format(), decoded.width(), decoded.height()));
        }

        let mut converted = frame::Video::empty();
        if let Some((scaler, ..)) = self.scaler.as_mut() {
            scaler.run(decoded, &mut converted)?;
        }

        let row_bytes = out_width as usize * 4;
        let stride = converted.stride(0);
        let data = converted.data(0);
        let mut rgba = Vec::with_capacity(row_bytes * out_height as usize);
        for row in 0..out_height as usize {
            rgba.extend_from_slice(&data[stride * row..stride * row + row_bytes]);
        }

        Ok(PreviewFrame {
            pts,
            time: Ticks::from_pts(pts, self.core_time_base),
            width: out_width,
            height: out_height,
            rgba,
            planes: None,
        })
    }
}

/// Tells the converter how the video's colours are encoded. Without this it assumes
/// the standard for old low-resolution video, which shifts the colours of HD footage.
fn set_color_conversion(context: &mut scaling::Context, decoded: &frame::Video) {
    let standard = match decoded.color_space() {
        color::Space::BT470BG | color::Space::SMPTE170M => ffi::SWS_CS_ITU601,
        color::Space::BT2020NCL | color::Space::BT2020CL => ffi::SWS_CS_BT2020,
        color::Space::BT709 => ffi::SWS_CS_ITU709,
        // Unlabelled video follows the usual convention for its resolution.
        _ if decoded.height() >= 720 => ffi::SWS_CS_ITU709,
        _ => ffi::SWS_CS_ITU601,
    };
    let full_range = i32::from(decoded.color_range() == color::Range::JPEG);
    // SAFETY: the context is live, and these calls only read the constant coefficient
    // tables and copy their values into it.
    unsafe {
        let coefficients = ffi::sws_getCoefficients(standard);
        let output_coefficients = ffi::sws_getCoefficients(ffi::SWS_CS_DEFAULT);
        ffi::sws_setColorspaceDetails(
            context.as_mut_ptr(),
            coefficients,
            full_range,
            output_coefficients,
            1,
            0,
            1 << 16,
            1 << 16,
        );
    }
}

/// A block of decoded sound from one track, as stereo samples.
#[derive(Clone, Debug)]
pub struct AudioChunk {
    pub track: usize,
    /// The audio frame this chunk starts at, counted from the start of the file.
    pub start_frame: i64,
    pub samples: Vec<f32>,
}

/// What the audio decoder found next.
#[derive(Debug)]
pub enum AudioStep {
    Chunk(AudioChunk),
    /// Nothing decoded yet, so ask again.
    Pending,
    End,
}

struct AudioTrackDecoder {
    decoder: decoder::Audio,
    resampler: Option<resampling::Context>,
    time_base: Rational,
    finished: bool,
}

pub struct AudioPreview {
    input: format::context::Input,
    /// Tracks, with their stream positions in the file.
    tracks: Vec<(usize, AudioTrackDecoder)>,
    sample_rate: u32,
    sent_eof: bool,
    next_to_drain: usize,
}

impl AudioPreview {
    pub fn open(path: &Path, sample_rate: u32) -> Result<Self, MediaError> {
        crate::init()?;
        let mut input = format::input(path)?;
        let mut tracks = Vec::new();
        for stream in input.streams() {
            if stream.parameters().medium() != Type::Audio {
                continue;
            }
            let mut audio = codec::context::Context::from_parameters(stream.parameters())?
                .decoder()
                .audio()?;
            audio.set_packet_time_base(stream.time_base());
            tracks.push((
                stream.index(),
                AudioTrackDecoder {
                    decoder: audio,
                    resampler: None,
                    time_base: stream.time_base(),
                    finished: false,
                },
            ));
        }
        let keep: Vec<usize> = tracks.iter().map(|(index, _)| *index).collect();
        discard_other_streams(&mut input, &keep);

        Ok(Self {
            input,
            tracks,
            sample_rate,
            sent_eof: false,
            next_to_drain: 0,
        })
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    pub fn seek(&mut self, seconds: f64) -> Result<(), MediaError> {
        if self.tracks.is_empty() {
            return Ok(());
        }
        seek_to_seconds(&mut self.input, seconds)?;
        for (_, track) in &mut self.tracks {
            track.decoder.flush();
            track.resampler = None;
            track.finished = false;
        }
        self.sent_eof = false;
        Ok(())
    }

    /// Which tracks have run out of sound.
    pub fn finished_tracks(&self) -> Vec<usize> {
        self.tracks
            .iter()
            .enumerate()
            .filter(|(_, (_, track))| track.finished)
            .map(|(position, _)| position)
            .collect()
    }

    pub fn next_step(&mut self) -> Result<AudioStep, MediaError> {
        if self.tracks.is_empty() {
            return Ok(AudioStep::End);
        }

        // Anything already decoded comes first.
        for offset in 0..self.tracks.len() {
            let position = (self.next_to_drain + offset) % self.tracks.len();
            if let Some(chunk) = self.drain_track(position)? {
                self.next_to_drain = position;
                return Ok(AudioStep::Chunk(chunk));
            }
        }

        if self.sent_eof {
            for (_, track) in &mut self.tracks {
                track.finished = true;
            }
            return Ok(AudioStep::End);
        }

        let mut packet = Packet::empty();
        match packet.read(&mut self.input) {
            Ok(()) => {
                let stream = packet.stream();
                if let Some((_, track)) = self.tracks.iter_mut().find(|(index, _)| *index == stream)
                {
                    // A damaged packet should cost a moment of sound, not the whole preview.
                    let _ = track.decoder.send_packet(&packet);
                }
            }
            Err(ffmpeg::Error::Eof) => {
                for (_, track) in &mut self.tracks {
                    track.decoder.send_eof()?;
                }
                self.sent_eof = true;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(AudioStep::Pending)
    }

    fn drain_track(&mut self, position: usize) -> Result<Option<AudioChunk>, MediaError> {
        let sample_rate = self.sample_rate;
        let track = &mut self.tracks[position].1;
        let mut decoded = frame::Audio::empty();
        if track.decoder.receive_frame(&mut decoded).is_err() {
            return Ok(None);
        }

        // Every block needs labelling, or one with no layout of its own stops matching the first.
        let layout = if decoded.channel_layout().is_empty() {
            ChannelLayout::default(i32::from(decoded.channels()))
        } else {
            decoded.channel_layout()
        };
        decoded.set_channel_layout(layout);
        if track.resampler.is_none() {
            track.resampler = Some(resampling::Context::get(
                decoded.format(),
                layout,
                decoded.rate(),
                Sample::F32(SampleType::Packed),
                ChannelLayout::STEREO,
                sample_rate,
            )?);
        }

        let pts = decoded.timestamp().unwrap_or_default();
        let seconds = f64::from(track.time_base) * pts as f64;
        let start_frame = (seconds * f64::from(sample_rate)).round() as i64;

        let mut converted = frame::Audio::empty();
        if let Some(resampler) = track.resampler.as_mut() {
            resampler.run(&decoded, &mut converted)?;
        }
        let frames = converted.samples();
        let bytes = converted.data(0);
        let samples = bytes[..frames * 2 * size_of::<f32>()]
            .as_chunks::<{ size_of::<f32>() }>()
            .0
            .iter()
            .map(|sample| f32::from_ne_bytes(*sample))
            .collect();

        Ok(Some(AudioChunk {
            track: position,
            start_frame,
            samples,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_video_is_scaled_to_fit() {
        assert_eq!(fit_preview_size(2560, 1440, 1280, 720), (1280, 720));
    }

    #[test]
    fn the_narrower_limit_decides_the_size() {
        assert_eq!(fit_preview_size(1000, 2000, 1280, 720), (360, 720));
    }

    #[test]
    fn small_video_is_never_scaled_up() {
        assert_eq!(fit_preview_size(320, 180, 1280, 720), (320, 180));
    }

    #[test]
    fn sizes_are_always_even() {
        let (width, height) = fit_preview_size(1921, 1081, 1000, 1000);

        assert_eq!(width % 2, 0);
        assert_eq!(height % 2, 0);
    }

    #[test]
    fn broken_sizes_do_not_crash() {
        assert_eq!(fit_preview_size(0, 0, 1280, 720), (2, 2));
    }
}
