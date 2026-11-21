//! Cuts a stretch out of a sound-only file into a file of the same kind.
//!
//! Compressed sound such as MP3, AAC, Ogg, Opus and WMA is cut by copying whole packets, so
//! nothing is re-encoded. A packet is 20 to 26 milliseconds, and the kept part covers both
//! ends, so it starts at most one packet before the start and ends at most one after the end.
//! Uncompressed WAV and AIFF are cut to the exact sample by slicing their packets, and FLAC is
//! decoded and written again, which loses nothing.

use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use bareclip_core::{
    time::{Ticks, TimeBase},
    timeline::TimeRange,
};
use ffmpeg_next::{
    self as ffmpeg, Packet, Rational, codec, encoder, format, format::Sample, frame, media::Type,
};

use crate::{error::MediaError, probe::to_time_base};

/// How a sound file was cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundCut {
    /// Whole packets copied, landing within one packet of each end.
    Packets,
    /// Cut to the exact sample, losing nothing.
    Exact,
}

/// The muxer that writes this kind of sound file, by its extension.
fn muxer_for(path: &Path) -> Option<&'static str> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)?;
    Some(match extension.as_str() {
        "mp3" => "mp3",
        "aac" => "adts",
        "m4a" => "ipod",
        "ogg" => "ogg",
        "opus" => "opus",
        "wma" => "asf",
        "mka" => "matroska",
        "wav" => "wav",
        "aiff" => "aiff",
        "flac" => "flac",
        _ => return None,
    })
}

/// Cuts `range` of a sound file into `output`, the same kind of file. A failed or cancelled
/// cut leaves no file behind.
pub fn cut_sound(
    input: &Path,
    output: &Path,
    range: TimeRange,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<SoundCut, MediaError> {
    crate::init()?;
    if crate::same_file(input, output) {
        return Err(MediaError::SameInputAndOutput);
    }
    let result = cut(input, output, range, cancel, progress);
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

fn cut(
    input_path: &Path,
    output_path: &Path,
    range: TimeRange,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<SoundCut, MediaError> {
    let mut input = format::input(input_path)?;
    let sound = input
        .streams()
        .best(Type::Audio)
        .ok_or(MediaError::NoVideoStream)?;
    let index = sound.index();
    let codec_id = sound.parameters().id();
    let muxer = muxer_for(output_path).ok_or(ffmpeg::Error::MuxerNotFound)?;
    let mut output = format::output_as(output_path, muxer)?;
    if codec_id == codec::Id::FLAC {
        decode_and_write_flac(&mut input, index, &mut output, range, cancel, progress)?;
        return Ok(SoundCut::Exact);
    }
    let exact = is_pcm(codec_id);
    copy_packets(
        &mut input,
        index,
        &mut output,
        range,
        cancel,
        progress,
        exact,
    )?;
    Ok(if exact {
        SoundCut::Exact
    } else {
        SoundCut::Packets
    })
}

fn is_pcm(id: codec::Id) -> bool {
    id.name().starts_with("pcm_")
}

/// Copies the packets that overlap the range, starting the copy at zero. Uncompressed packets
/// are sliced to the exact sample.
#[allow(clippy::too_many_arguments)]
fn copy_packets(
    input: &mut format::context::Input,
    index: usize,
    output: &mut format::context::Output,
    range: TimeRange,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
    exact: bool,
) -> Result<(), MediaError> {
    let (time_base, parameters, metadata, sample_rate, block) = {
        let stream = input.stream(index).ok_or(MediaError::NoVideoStream)?;
        let parameters = stream.parameters();
        // SAFETY: the parameters belong to a live stream, and only plain fields are read.
        let raw = unsafe { &*parameters.as_ptr() };
        let bits = unsafe { ffmpeg::ffi::av_get_bits_per_sample(raw.codec_id) };
        let block = (bits.max(0) as usize / 8) * raw.ch_layout.nb_channels.max(0) as usize;
        (
            stream.time_base(),
            parameters,
            stream.metadata().to_owned(),
            raw.sample_rate.max(1),
            block,
        )
    };
    {
        let mut added = output.add_stream(encoder::find(codec::Id::None))?;
        added.set_parameters(parameters);
        added.set_time_base(time_base);
        added.set_metadata(metadata);
        // SAFETY: a plain field the muxer reads; zero lets it pick its own tag.
        unsafe {
            (*added.parameters().as_mut_ptr()).codec_tag = 0;
        }
    }
    output.set_metadata(input.metadata().to_owned());
    output.write_header()?;
    let out_base = output
        .stream(0)
        .map_or(Rational(1, sample_rate), |stream| stream.time_base());
    let base = to_time_base(time_base);
    let start_pts = range.start.to_pts(base);
    let end_pts = range.end.to_pts(base);
    crate::preview::seek_to_seconds(input, range.start.as_seconds())?;
    let samples_per_pts = f64::from(sample_rate) * f64::from(time_base.numerator())
        / f64::from(time_base.denominator());

    let mut packet = Packet::empty();
    let mut first_pts: Option<i64> = None;
    let mut next_pts = 0_i64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        match packet.read(input) {
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(error.into()),
        }
        if packet.stream() != index {
            continue;
        }
        let Some(pts) = packet.pts().or(packet.dts()) else {
            continue;
        };
        let length = packet.duration().max(0);
        if pts + length.max(1) <= start_pts {
            continue;
        }
        if pts >= end_pts {
            break;
        }
        let mut written = packet.clone();
        if exact && block > 0 {
            // Slice the samples to the range, which loses nothing in uncompressed sound.
            let data = packet.data().unwrap_or_default();
            let samples = data.len() / block;
            let skip = (((start_pts - pts).max(0) as f64) * samples_per_pts).round() as usize;
            let until = (((end_pts - pts) as f64) * samples_per_pts).round() as usize;
            let (skip, until) = (skip.min(samples), until.min(samples));
            if until <= skip {
                continue;
            }
            written = Packet::copy(&data[skip * block..until * block]);
            let kept = (until - skip) as f64 / samples_per_pts;
            written.set_pts(Some(next_pts));
            written.set_dts(Some(next_pts));
            written.set_duration(kept.round() as i64);
            next_pts += kept.round() as i64;
        } else {
            let from = *first_pts.get_or_insert(pts);
            written.set_pts(Some(pts - from));
            written.set_dts(packet.dts().map(|dts| dts - from));
        }
        written.set_stream(0);
        written.rescale_ts(time_base, out_base);
        written.set_position(-1);
        written.write_interleaved(output)?;
        let share = ((pts - start_pts) as f64 / (end_pts - start_pts).max(1) as f64) as f32;
        progress(share.clamp(0.0, 1.0));
    }
    output.write_trailer()?;
    progress(1.0);
    Ok(())
}

/// Decodes FLAC and writes the range again as FLAC, to the exact sample. FLAC loses nothing,
/// so the samples come out the same.
fn decode_and_write_flac(
    input: &mut format::context::Input,
    index: usize,
    output: &mut format::context::Output,
    range: TimeRange,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<(), MediaError> {
    let (time_base, parameters) = {
        let stream = input.stream(index).ok_or(MediaError::NoVideoStream)?;
        (stream.time_base(), stream.parameters())
    };
    let mut decoder = codec::context::Context::from_parameters(parameters)?
        .decoder()
        .audio()?;
    decoder.set_packet_time_base(time_base);
    let rate = decoder.rate();
    let channels = usize::from(decoder.channels());
    // Twenty-four bit FLAC decodes into 32-bit values, which FLAC writes back as they were.
    let wide = matches!(
        decoder.format(),
        Sample::I32(_) | Sample::F32(_) | Sample::F64(_)
    );
    let bits = unsafe { (*decoder.as_ptr()).bits_per_raw_sample };
    let codec = encoder::find(codec::Id::FLAC).ok_or(ffmpeg::Error::EncoderNotFound)?;
    let mut context = codec::context::Context::new_with_codec(codec)
        .encoder()
        .audio()?;
    context.set_rate(rate as i32);
    context.set_channel_layout(decoder_layout(channels));
    let format = if wide {
        Sample::I32(format::sample::Type::Packed)
    } else {
        Sample::I16(format::sample::Type::Packed)
    };
    context.set_format(format);
    context.set_time_base(Rational(1, rate as i32));
    // SAFETY: a plain field the encoder reads when it opens.
    unsafe {
        (*context.as_mut_ptr()).bits_per_raw_sample = bits;
    }
    let mut encoder = context.open_as(codec)?;
    let frame_size = (encoder.frame_size() as usize).max(1024);
    {
        let mut added = output.add_stream(codec)?;
        added.set_parameters(&encoder);
        added.set_time_base(Rational(1, rate as i32));
    }
    output.set_metadata(input.metadata().to_owned());
    output.write_header()?;
    let out_base = output
        .stream(0)
        .map_or(Rational(1, rate as i32), |stream| stream.time_base());

    let start_sample = (range.start.as_seconds() * f64::from(rate)).round() as i64;
    let end_sample = (range.end.as_seconds() * f64::from(rate)).round() as i64;
    crate::preview::seek_to_seconds(input, range.start.as_seconds())?;
    let samples_base = TimeBase {
        num: 1,
        den: rate as i32,
    };

    // Interleaved samples waiting to fill a whole frame for the encoder.
    let mut waiting: Vec<i32> = Vec::new();
    let mut written = 0_i64;
    let send = |encoder: &mut encoder::Audio,
                waiting: &mut Vec<i32>,
                written: &mut i64,
                output: &mut format::context::Output,
                all: bool|
     -> Result<(), MediaError> {
        while waiting.len() >= frame_size * channels || (all && !waiting.is_empty()) {
            let count = (waiting.len() / channels).min(frame_size);
            let mut frame = frame::Audio::new(format, count, decoder_layout(channels));
            frame.set_rate(rate);
            let taken: Vec<i32> = waiting.drain(..count * channels).collect();
            let data = frame.data_mut(0);
            if wide {
                for (to, sample) in data.as_chunks_mut::<4>().0.iter_mut().zip(&taken) {
                    *to = sample.to_le_bytes();
                }
            } else {
                for (to, sample) in data.as_chunks_mut::<2>().0.iter_mut().zip(&taken) {
                    *to = (*sample as i16).to_le_bytes();
                }
            }
            frame.set_pts(Some(*written));
            *written += count as i64;
            encoder.send_frame(&frame)?;
            drain(encoder, output, out_base, rate)?;
        }
        Ok(())
    };

    let mut decoded = frame::Audio::empty();
    let mut packet = Packet::empty();
    let mut finished = false;
    while !finished {
        if cancel.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        let ended = match packet.read(input) {
            Ok(()) => false,
            Err(ffmpeg::Error::Eof) => true,
            Err(error) => return Err(error.into()),
        };
        if ended {
            decoder.send_eof()?;
        } else if packet.stream() == index {
            decoder.send_packet(&packet)?;
        } else {
            continue;
        }
        while decoder.receive_frame(&mut decoded).is_ok() {
            let first = Ticks::from_pts(decoded.pts().unwrap_or(0), to_time_base(time_base));
            let first = first.to_pts(samples_base);
            let interleaved = interleave(&decoded, channels);
            for (offset, chunk) in interleaved.chunks(channels).enumerate() {
                let at = first + offset as i64;
                if at >= start_sample && at < end_sample {
                    waiting.extend_from_slice(chunk);
                }
            }
            if first + decoded.samples() as i64 >= end_sample {
                finished = true;
            }
            let share = (first - start_sample) as f64 / (end_sample - start_sample).max(1) as f64;
            progress((share as f32).clamp(0.0, 1.0));
            send(&mut encoder, &mut waiting, &mut written, output, false)?;
        }
        if ended {
            finished = true;
        }
    }
    send(&mut encoder, &mut waiting, &mut written, output, true)?;
    encoder.send_eof()?;
    drain(&mut encoder, output, out_base, rate)?;
    output.write_trailer()?;
    progress(1.0);
    Ok(())
}

fn decoder_layout(channels: usize) -> ffmpeg::ChannelLayout {
    ffmpeg::ChannelLayout::default(channels as i32)
}

fn drain(
    encoder: &mut encoder::Audio,
    output: &mut format::context::Output,
    out_base: Rational,
    rate: u32,
) -> Result<(), MediaError> {
    let mut packet = Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(0);
        packet.rescale_ts(Rational(1, rate as i32), out_base);
        packet.write_interleaved(output)?;
    }
    Ok(())
}

/// A decoded frame's samples, every channel's in turn, as whole numbers at their own depth.
fn interleave(frame: &frame::Audio, channels: usize) -> Vec<i32> {
    let count = frame.samples();
    let mut samples = Vec::with_capacity(count * channels);
    let read = |plane: usize, at: usize, wide: bool| -> i32 {
        let data = frame.data(plane);
        if wide {
            let start = at * 4;
            i32::from_le_bytes([
                data[start],
                data[start + 1],
                data[start + 2],
                data[start + 3],
            ])
        } else {
            let start = at * 2;
            i32::from(i16::from_le_bytes([data[start], data[start + 1]]))
        }
    };
    let (wide, planar) = match frame.format() {
        Sample::I32(kind) => (true, kind == format::sample::Type::Planar),
        Sample::I16(kind) => (false, kind == format::sample::Type::Planar),
        _ => (true, false),
    };
    for sample in 0..count {
        for channel in 0..channels {
            samples.push(if planar {
                read(channel, sample, wide)
            } else {
                read(0, sample * channels + channel, wide)
            });
        }
    }
    samples
}

#[cfg(test)]
mod tests;
