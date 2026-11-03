//! Writes the kept sections of a video to a new file.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use bareclip_core::{
    splice::{CutMode, CutPlan, FrameIndex, Piece, Section},
    time::{Ticks, TimeBase},
    timeline::TimeRange,
};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{
    Dictionary, Packet, Rational, codec, codec::Id, decoder, encoder, ffi, format, frame,
    media::Type, picture,
};

use crate::{
    error::MediaError,
    media_foundation::OpenEncoder,
    nal,
    probe::{discard_other_streams, join_problem, probe, to_time_base},
};

const SEEK_MARGIN_SECONDS: i64 = 2;
/// How far past a section to keep reading while waiting for its last audio.
const AUDIO_READ_AHEAD_SECONDS: f64 = 10.0;

/// A stretch of a source video to include, in playing order.
#[derive(Clone, Debug)]
pub struct ExportPart {
    pub input: PathBuf,
    pub frames: Arc<FrameIndex>,
    pub range: TimeRange,
    /// Dead space on the timeline before this part, written as black and silence.
    pub gap_before: Ticks,
}

#[derive(Clone, Debug)]
pub struct ExportRequest {
    pub output: PathBuf,
    pub parts: Vec<ExportPart>,
    pub mode: CutMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportReport {
    pub mode_requested: CutMode,
    pub mode_used: CutMode,
    pub sections: usize,
    pub copied_frames: usize,
    pub encoded_frames: usize,
    /// Black frames written for dead space on the timeline.
    pub black_frames: usize,
    /// The encoder used for the re-encoded parts, if any were needed.
    pub encoder: Option<String>,
    pub audio_tracks: usize,
    pub elapsed: Duration,
}

fn rescale(value: i64, from: Rational, to: Rational) -> i64 {
    let numerator = i128::from(value) * i128::from(from.numerator()) * i128::from(to.denominator());
    let denominator = i128::from(from.denominator()) * i128::from(to.numerator());
    let half = denominator / 2;
    let rounded = if numerator >= 0 {
        (numerator + half) / denominator
    } else {
        (numerator - half) / denominator
    };
    rounded as i64
}

/// The encoders to try for a codec, fastest first. Hardware comes first because
/// it is quick, and these are short sections where speed matters most.
///
/// Only codecs proven to join cleanly with re-encoded parts are listed. MPEG-4 Part
/// 2, for one, repeats and drops frames at the join when it has B-frames, so other
/// codecs get keyframe cuts instead.
fn encoder_candidates(codec: Id) -> Vec<String> {
    let names: &[&str] = match codec {
        Id::H264 => &[
            "h264_nvenc",
            "h264_amf",
            "h264_qsv",
            "h264_mf",
            "libopenh264",
        ],
        Id::HEVC => &[
            "hevc_nvenc",
            "hevc_amf",
            "hevc_qsv",
            "hevc_mf",
            "libkvazaar",
        ],
        Id::AV1 => &["av1_nvenc", "av1_amf", "av1_qsv", "libsvtav1"],
        _ => &[],
    };
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// Settings that aim to keep re-encoded sections close to the source's quality,
/// without B-frames so the output order is the same as the display order.
fn encoder_options(name: &str) -> Dictionary<'static> {
    let mut options = Dictionary::new();
    let settings: &[(&str, &str)] = if name.ends_with("_nvenc") {
        &[
            ("preset", "p4"),
            ("tune", "hq"),
            ("rc", "constqp"),
            ("qp", "18"),
        ]
    } else if name.ends_with("_amf") {
        &[
            ("usage", "transcoding"),
            ("rc", "cqp"),
            ("qp_i", "18"),
            ("qp_p", "18"),
        ]
    } else if name.ends_with("_qsv") {
        &[("global_quality", "18")]
    } else if name == "libsvtav1" {
        &[("crf", "24"), ("preset", "8")]
    } else if name == "libkvazaar" {
        &[("kvazaar-params", "qp=18,bipred=0,gop=0")]
    } else {
        &[]
    };
    for (key, value) in settings {
        options.set(key, value);
    }
    options
}

struct SourceVideo {
    index: usize,
    time_base: Rational,
    parameters: codec::Parameters,
    frame_rate: Rational,
    bit_rate: usize,
}

fn open_encoder(
    name: &str,
    source: &SourceVideo,
    decoder: &decoder::Video,
) -> Result<OpenEncoder, ffmpeg::Error> {
    let found = encoder::find_by_name(name).ok_or(ffmpeg::Error::EncoderNotFound)?;
    let mut video = codec::context::Context::new_with_codec(found)
        .encoder()
        .video()?;
    video.set_width(decoder.width());
    video.set_height(decoder.height());
    video.set_format(decoder.format());
    video.set_aspect_ratio(decoder.aspect_ratio());
    video.set_time_base(source.time_base);
    video.set_frame_rate(Some(source.frame_rate));
    video.set_colorspace(decoder.color_space());
    video.set_color_range(decoder.color_range());
    // SAFETY: both contexts are live, and these are plain fields read when the
    // encoder opens. The safe wrappers do not offer them.
    unsafe {
        (*video.as_mut_ptr()).color_primaries = decoder.color_primaries().into();
        (*video.as_mut_ptr()).color_trc = decoder.color_transfer_characteristic().into();
    }
    video.set_max_b_frames(0);
    video.set_gop(u32::MAX >> 8);
    // Encoders with a quality setting reject or ignore a target bitrate.
    let options = encoder_options(name);
    if options.iter().next().is_none() {
        video.set_bit_rate(source.bit_rate);
    }
    // Leaving global headers off puts the settings inside the stream, so each
    // re-encoded section carries what a decoder needs to read it.
    OpenEncoder::open(video, name, options)
}

fn new_decoder(source: &SourceVideo) -> Result<decoder::Video, ffmpeg::Error> {
    let mut context = codec::context::Context::from_parameters(source.parameters.clone())?;
    context.set_threading(codec::threading::Config {
        kind: codec::threading::Type::Frame,
        count: 0,
    });
    let mut video = context.decoder().video()?;
    video.set_packet_time_base(source.time_base);
    Ok(video)
}

/// Finds the first encoder that accepts this video. Returns its name, or `None`
/// if nothing here can match the source.
fn choose_encoder(codec: Id, source: &SourceVideo) -> Option<String> {
    let decoder = new_decoder(source).ok()?;
    encoder_candidates(codec)
        .into_iter()
        .find(|name| open_encoder(name, source, &decoder).is_ok())
}

/// How re-encoded data is packaged to match the copied stream around it.
#[derive(Clone, Debug)]
struct Packaging {
    length_size: Option<usize>,
    /// The source's decoder settings, ready to place before a copied frame.
    source_settings: Vec<u8>,
}

fn packaging_for(codec: Id, extradata: &[u8]) -> Packaging {
    let length_prefixed = nal::is_length_prefixed(extradata);
    match codec {
        Id::H264 if length_prefixed => {
            let length_size = nal::avc_length_size(extradata);
            Packaging {
                length_size,
                source_settings: nal::avc_parameter_sets(extradata)
                    .map(|units| nal::join_units(&units, length_size))
                    .unwrap_or_default(),
            }
        }
        Id::HEVC if length_prefixed => {
            let length_size = nal::hevc_length_size(extradata);
            Packaging {
                length_size,
                source_settings: nal::hevc_parameter_sets(extradata)
                    .map(|units| nal::join_units(&units, length_size))
                    .unwrap_or_default(),
            }
        }
        Id::AV1 => Packaging {
            length_size: None,
            source_settings: extradata.get(4..).unwrap_or_default().to_vec(),
        },
        _ => Packaging {
            length_size: None,
            source_settings: extradata.to_vec(),
        },
    }
}

fn extradata_of(parameters: &codec::Parameters) -> Vec<u8> {
    // SAFETY: the pointer and size come from one live parameters object, and the
    // data is copied before that object can change.
    unsafe {
        let raw = parameters.as_ptr();
        let size = (*raw).extradata_size;
        if (*raw).extradata.is_null() || size <= 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts((*raw).extradata, size as usize).to_vec()
        }
    }
}

/// Moves timestamps from the source to their place in the output, and keeps the
/// stored order valid for the muxer.
struct VideoTimeline {
    /// How far the stored order runs ahead of the display order.
    delay: i64,
    last_dts: Option<i64>,
}

impl VideoTimeline {
    fn new(delay: i64) -> Self {
        Self {
            delay,
            last_dts: None,
        }
    }

    fn place(&mut self, pts: i64, dts: i64) -> (i64, i64) {
        let dts = match self.last_dts {
            Some(last) if dts <= last => last + 1,
            _ => dts,
        };
        self.last_dts = Some(dts);
        (pts.max(dts), dts)
    }
}

/// An audio track of the source being read.
struct SourceAudioTrack {
    output_index: usize,
    time_base: Rational,
    finished_section: bool,
}

/// The muxer and everything needed to place packets into it. All timestamps given
/// to it are already in the output's own time base.
struct Writer {
    output: format::context::Output,
    output_time_base: Vec<Rational>,
    video_timeline: VideoTimeline,
    /// Where the previous packet written to each output stream ended.
    next_free: Vec<Option<i64>>,
}

impl Writer {
    fn write(&mut self, packet: &mut Packet, output_index: usize) -> Result<(), ffmpeg::Error> {
        packet.set_position(-1);
        packet.set_stream(output_index);
        packet.write_interleaved(&mut self.output)
    }
}

/// A re-encoded piece in progress.
struct EncodeRun {
    decoder: decoder::Video,
    encoder: OpenEncoder,
    pts_start: i64,
    pts_end: i64,
    first_frame: bool,
}

impl EncodeRun {
    fn new(
        source: &SourceVideo,
        encoder_name: &str,
        pts_start: i64,
        pts_end: i64,
    ) -> Result<Self, ffmpeg::Error> {
        let decoder = new_decoder(source)?;
        let encoder = open_encoder(encoder_name, source, &decoder)?;
        Ok(Self {
            decoder,
            encoder,
            pts_start,
            pts_end,
            first_frame: true,
        })
    }
}

struct SectionWriter<'a> {
    source: &'a SourceVideo,
    packaging: &'a Packaging,
    encoder_name: Option<&'a str>,
    /// Display time in the source where this section starts.
    section_start: i64,
    /// Where this section starts in the output.
    output_start: i64,
    output_time_base: Rational,
    run: Option<EncodeRun>,
    /// A re-encoded part put different settings into the stream, so the next
    /// copied frame must bring back the source's settings.
    restore_settings: bool,
    durations: &'a HashMap<i64, i64>,
}

impl SectionWriter<'_> {
    /// Moves a source timestamp to its place in the output.
    fn to_output(&self, timestamp: i64) -> i64 {
        rescale(
            timestamp - self.section_start,
            self.source.time_base,
            self.output_time_base,
        ) + self.output_start
    }

    fn output_duration(&self, duration: i64) -> i64 {
        rescale(duration, self.source.time_base, self.output_time_base).max(1)
    }

    fn write_encoded(&mut self, writer: &mut Writer) -> Result<(), MediaError> {
        let mut encoded = Packet::empty();
        loop {
            let Some(run) = self.run.as_mut() else {
                return Ok(());
            };
            if run.encoder.receive_packet(&mut encoded).is_err() {
                return Ok(());
            }
            let Some(pts) = encoded.pts() else {
                continue;
            };
            let out_pts = self.to_output(pts);
            let duration = self.output_duration(self.durations.get(&pts).copied().unwrap_or(1));
            let (place_pts, place_dts) = writer
                .video_timeline
                .place(out_pts, out_pts - writer.video_timeline.delay);

            let mut packet = match self.packaging.length_size {
                Some(size) => Packet::copy(&nal::start_codes_to_length_prefixed(
                    encoded.data().unwrap_or_default(),
                    size,
                )),
                None => Packet::copy(encoded.data().unwrap_or_default()),
            };
            packet.set_flags(encoded.flags());
            packet.set_pts(Some(place_pts));
            packet.set_dts(Some(place_dts));
            packet.set_duration(duration);
            writer.write(&mut packet, 0)?;
        }
    }

    fn encode_decoded(&mut self, writer: &mut Writer) -> Result<(), MediaError> {
        let mut decoded = frame::Video::empty();
        loop {
            let Some(run) = self.run.as_mut() else {
                return Ok(());
            };
            if run.decoder.receive_frame(&mut decoded).is_err() {
                return Ok(());
            }
            let Some(pts) = decoded.timestamp() else {
                continue;
            };
            if pts < run.pts_start || pts >= run.pts_end {
                continue;
            }
            decoded.set_pts(Some(pts));
            decoded.set_kind(if run.first_frame {
                picture::Type::I
            } else {
                picture::Type::None
            });
            run.first_frame = false;
            run.encoder.send_frame(&decoded)?;
            self.write_encoded(writer)?;
        }
    }

    fn finish_run(&mut self, writer: &mut Writer) -> Result<(), MediaError> {
        if self.run.is_none() {
            return Ok(());
        }
        if let Some(run) = self.run.as_mut() {
            run.decoder.send_eof()?;
        }
        self.encode_decoded(writer)?;
        if let Some(run) = self.run.as_mut() {
            run.encoder.send_eof()?;
        }
        self.write_encoded(writer)?;
        self.run = None;
        self.restore_settings = true;
        Ok(())
    }

    fn start_piece(&mut self, piece: Piece) -> Result<(), MediaError> {
        if let Piece::Encode {
            pts_start, pts_end, ..
        } = piece
        {
            let name = self.encoder_name.ok_or(ffmpeg::Error::EncoderNotFound)?;
            self.run = Some(EncodeRun::new(self.source, name, pts_start, pts_end)?);
        }
        Ok(())
    }

    fn write_copied(&mut self, packet: &Packet, writer: &mut Writer) -> Result<(), MediaError> {
        let pts = packet.pts().unwrap_or_default();
        let dts = packet.dts().unwrap_or(pts);
        let (place_pts, place_dts) = writer
            .video_timeline
            .place(self.to_output(pts), self.to_output(dts));

        let body = packet.data().unwrap_or_default();
        let mut output = if self.restore_settings && !self.packaging.source_settings.is_empty() {
            let mut joined = self.packaging.source_settings.clone();
            joined.extend_from_slice(body);
            Packet::copy(&joined)
        } else {
            Packet::copy(body)
        };
        self.restore_settings = false;

        output.set_flags(packet.flags());
        output.set_pts(Some(place_pts));
        output.set_dts(Some(place_dts));
        output.set_duration(
            self.output_duration(
                self.durations
                    .get(&pts)
                    .copied()
                    .unwrap_or_else(|| packet.duration()),
            ),
        );
        writer.write(&mut output, 0)?;
        Ok(())
    }
}

/// A picture of pure black in the video's own format, whatever that is.
fn black_frame(decoder: &decoder::Video) -> Result<frame::Video, ffmpeg::Error> {
    let mut black = frame::Video::new(decoder.format(), decoder.width(), decoder.height());
    let mut line_sizes = [0isize; 4];
    // SAFETY: the frame was just allocated for this format and size, and FFmpeg writes
    // only inside its planes.
    let result = unsafe {
        let raw = black.as_mut_ptr();
        for (size, source) in line_sizes.iter_mut().zip((*raw).linesize.iter()) {
            *size = *source as isize;
        }
        ffi::av_image_fill_black(
            (*raw).data.as_ptr().cast(),
            line_sizes.as_ptr(),
            decoder.format().into(),
            decoder.color_range().into(),
            decoder.width() as i32,
            decoder.height() as i32,
        )
    };
    if result < 0 {
        return Err(ffmpeg::Error::from(result));
    }
    Ok(black)
}

/// How many frames fill a stretch of dead space. Less than half a frame needs none.
fn black_frame_count(gap: Ticks, frame_rate: Rational) -> usize {
    (gap.as_seconds() * f64::from(frame_rate)).round().max(0.0) as usize
}

/// Writes dead space as black frames, with the same encoder that cut points use.
fn write_black(
    group: &SourceGroup,
    job: &ExportJob,
    writer: &mut Writer,
    progress: &mut Progress,
    cursor: Ticks,
) -> Result<(), MediaError> {
    let source = &group.video;
    let frames = group.black_frames();
    let encoder_name = job.encoder_name.ok_or(MediaError::GapNeedsEncoder)?;
    let packaging = packaging_for(source.parameters.id(), &extradata_of(&source.parameters));
    let frame_rate = source.frame_rate;
    let frame_seconds = Rational(frame_rate.denominator(), frame_rate.numerator());
    let pts_of = |number: usize| rescale(number as i64, frame_seconds, source.time_base);
    let durations: HashMap<i64, i64> = (0..frames)
        .map(|number| (pts_of(number), (pts_of(number + 1) - pts_of(number)).max(1)))
        .collect();
    let run = EncodeRun::new(source, encoder_name, 0, pts_of(frames))?;
    let mut black = black_frame(&run.decoder)?;
    let output_time_base = writer.output_time_base[0];
    let mut section_writer = SectionWriter {
        source,
        packaging: &packaging,
        encoder_name: Some(encoder_name),
        section_start: 0,
        output_start: cursor.to_pts(to_time_base(output_time_base)),
        output_time_base,
        run: Some(run),
        restore_settings: true,
        durations: &durations,
    };
    for number in 0..frames {
        if job.cancel.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        black.set_pts(Some(pts_of(number)));
        black.set_kind(if number == 0 {
            picture::Type::I
        } else {
            picture::Type::None
        });
        if let Some(run) = section_writer.run.as_mut() {
            run.encoder.send_frame(&black)?;
        }
        section_writer.write_encoded(writer)?;
        progress.tick();
    }
    section_writer.finish_run(writer)
}

/// Encodes silent packets in an audio track's own format.
fn silent_packets(
    parameters: &codec::Parameters,
    wanted_samples: i64,
) -> Result<(Vec<Packet>, i64, i32), ffmpeg::Error> {
    let source = codec::context::Context::from_parameters(parameters.clone())?
        .decoder()
        .audio()?;
    let found = encoder::find(parameters.id()).ok_or(ffmpeg::Error::EncoderNotFound)?;
    let format = found
        .audio()?
        .formats()
        .and_then(|mut formats| formats.next())
        .ok_or(ffmpeg::Error::EncoderNotFound)?;
    let mut audio = codec::context::Context::new_with_codec(found)
        .encoder()
        .audio()?;
    let rate = source.rate() as i32;
    audio.set_rate(rate);
    audio.set_channel_layout(source.channel_layout());
    audio.set_format(format);
    audio.set_time_base(Rational(1, rate));
    let mut audio = audio.open()?;

    let frame_size = match audio.frame_size() {
        0 => 1024,
        size => i64::from(size),
    };
    let wanted = wanted_samples / frame_size;
    let mut silence = frame::Audio::new(format, frame_size as usize, source.channel_layout());
    silence.set_rate(rate as u32);
    for plane in 0..silence.planes() {
        silence.data_mut(plane).fill(0);
    }

    let mut packets = Vec::new();
    let mut sent = 0;
    let mut encoded = Packet::empty();
    // Encoders hold on to some samples at first, so this keeps going until enough
    // packets have come out.
    while (packets.len() as i64) < wanted && sent < wanted + 16 {
        silence.set_pts(Some(sent * frame_size));
        audio.send_frame(&silence)?;
        sent += 1;
        while audio.receive_packet(&mut encoded).is_ok() {
            packets.push(Packet::copy(encoded.data().unwrap_or_default()));
        }
    }
    packets.truncate(wanted.max(0) as usize);
    Ok((packets, frame_size, rate))
}

/// Writes silence into every audio track up to the end of some dead space. A track
/// whose format has no encoder here is left empty for that stretch. Its later packets
/// still carry the right times, so it stays in sync and players treat the hole as quiet.
fn write_silence(until: Ticks, writer: &mut Writer) -> Result<(), MediaError> {
    for track in 1..writer.output_time_base.len() {
        let time_base = writer.output_time_base[track];
        let parameters = writer
            .output
            .stream(track)
            .ok_or(MediaError::NoVideoStream)?
            .parameters();
        let start = writer.next_free[track].unwrap_or(0);
        let end = until.to_pts(to_time_base(time_base));
        let room = end - start;
        if room <= 0 {
            continue;
        }
        let rate = codec::context::Context::from_parameters(parameters.clone())
            .and_then(|context| context.decoder().audio())
            .map(|audio| audio.rate() as i32);
        let Ok(rate) = rate else {
            continue;
        };
        let samples = rescale(room, time_base, Rational(1, rate));
        let Ok((packets, frame_size, rate)) = silent_packets(&parameters, samples) else {
            continue;
        };
        let mut pts_samples = 0;
        for mut packet in packets {
            let pts = start + rescale(pts_samples, Rational(1, rate), time_base);
            let next = start + rescale(pts_samples + frame_size, Rational(1, rate), time_base);
            packet.set_flags(codec::packet::Flags::KEY);
            packet.set_pts(Some(pts));
            packet.set_dts(Some(pts));
            packet.set_duration((next - pts).max(1));
            writer.write(&mut packet, track)?;
            writer.next_free[track] = Some(next);
            pts_samples += frame_size;
        }
    }
    Ok(())
}

fn piece_end(piece: Piece) -> usize {
    match piece {
        Piece::Copy { end, .. } => end,
        Piece::Encode { decode_end, .. } => decode_end,
    }
}

fn piece_first(piece: Piece) -> usize {
    match piece {
        Piece::Copy { first, .. } => first,
        Piece::Encode { decode_first, .. } => decode_first,
    }
}

/// Copies a stream's identity from the source, such as its language and title.
fn copy_stream_identity(input: &format::stream::Stream, output: &mut format::stream::StreamMut) {
    output.set_metadata(input.metadata().to_owned());
    // SAFETY: both pointers come from live streams, and `disposition` is a plain
    // integer field that the muxer reads later.
    unsafe {
        (*output.as_mut_ptr()).disposition = (*input.as_ptr()).disposition;
    }
}

struct Progress<'a> {
    report: &'a dyn Fn(f32),
    total: usize,
    done: usize,
}

impl Progress<'_> {
    fn tick(&mut self) {
        self.done += 1;
        if self.done.is_multiple_of(32) {
            (self.report)(self.done as f32 / self.total.max(1) as f32);
        }
    }
}

/// Consecutive parts from one source file, ready to write.
struct SourceGroup {
    input: PathBuf,
    frames: Arc<FrameIndex>,
    gap_before: Ticks,
    video: SourceVideo,
    plan: CutPlan,
}

impl SourceGroup {
    fn black_frames(&self) -> usize {
        black_frame_count(self.gap_before, self.video.frame_rate)
    }
}

/// Everything one run of the export needs.
struct ExportJob<'a> {
    request: &'a ExportRequest,
    temporary: &'a Path,
    groups: &'a [SourceGroup],
    encoder_name: Option<&'a str>,
    cancel: &'a AtomicBool,
    progress: &'a dyn Fn(f32),
}

fn clear_codec_tag(stream: &mut format::stream::StreamMut) {
    // SAFETY: the parameters belong to this stream, and `codec_tag` is a plain field
    // the muxer reads when writing the header.
    unsafe {
        (*stream.parameters().as_mut_ptr()).codec_tag = 0;
    }
}

/// Sets up the output's streams from the first source. Every other source has been
/// checked to match it.
fn open_output(job: &ExportJob) -> Result<(format::context::Output, usize), MediaError> {
    let first = &job.groups[0];
    let input = format::input(&first.input)?;
    let mut output = format::output(job.temporary)?;
    let same_container = first.input.extension() == job.request.output.extension();
    let rewrites_video = job.groups.len() > 1
        || job
            .groups
            .iter()
            .any(|g| g.plan.encoded_frames > 0 || g.black_frames() > 0);

    // The video always becomes output stream 0.
    {
        let stream = input
            .stream(first.video.index)
            .ok_or(MediaError::NoVideoStream)?;
        let mut added = output.add_stream(encoder::find(Id::None))?;
        added.set_parameters(stream.parameters());
        added.set_time_base(stream.time_base());
        added.set_avg_frame_rate(stream.avg_frame_rate());
        copy_stream_identity(&stream, &mut added);
        // A file header type that promises settings live only in the header would
        // be untrue once other parts carry their own, so the muxer picks again.
        if !same_container || rewrites_video {
            clear_codec_tag(&mut added);
        }
    }

    let mut audio_count = 0;
    for stream in input.streams() {
        if stream.parameters().medium() != Type::Audio {
            continue;
        }
        let mut added = output.add_stream(encoder::find(Id::None))?;
        added.set_parameters(stream.parameters());
        added.set_time_base(stream.time_base());
        copy_stream_identity(&stream, &mut added);
        if !same_container {
            clear_codec_tag(&mut added);
        }
        audio_count += 1;
    }

    output.set_metadata(input.metadata().to_owned());
    Ok((output, audio_count))
}

fn run_export(job: &ExportJob) -> Result<usize, MediaError> {
    let (mut output, audio_count) = open_output(job)?;
    output.write_header()?;

    let output_time_base: Vec<Rational> =
        output.streams().map(|stream| stream.time_base()).collect();
    let delay = job
        .groups
        .iter()
        .map(|group| {
            let most = group
                .frames
                .frames()
                .iter()
                .map(|frame| frame.pts - frame.dts)
                .max()
                .unwrap_or(0);
            rescale(most.max(0), group.video.time_base, output_time_base[0])
        })
        .max()
        .unwrap_or(0);
    let mut writer = Writer {
        next_free: vec![None; output_time_base.len()],
        output,
        output_time_base,
        video_timeline: VideoTimeline::new(delay),
    };

    let total_frames: usize = job
        .groups
        .iter()
        .flat_map(|group| &group.plan.sections)
        .flat_map(|section| &section.pieces)
        .map(|&piece| piece_end(piece) - piece_first(piece))
        .sum::<usize>()
        + job
            .groups
            .iter()
            .map(SourceGroup::black_frames)
            .sum::<usize>();
    let mut progress = Progress {
        report: job.progress,
        total: total_frames,
        done: 0,
    };

    let mut cursor = Ticks(0);
    let mut restore_settings = false;
    for (position, group) in job.groups.iter().enumerate() {
        // A new source file means new decoder settings for whatever follows.
        restore_settings |= position > 0;
        if group.black_frames() > 0 {
            write_black(group, job, &mut writer, &mut progress, cursor)?;
            cursor = Ticks(cursor.0 + group.gap_before.0);
            write_silence(cursor, &mut writer)?;
            restore_settings = true;
        }
        write_group(
            group,
            job,
            &mut writer,
            &mut progress,
            &mut cursor,
            &mut restore_settings,
        )?;
    }

    writer.output.write_trailer()?;
    Ok(audio_count)
}

fn write_group(
    group: &SourceGroup,
    job: &ExportJob,
    writer: &mut Writer,
    progress: &mut Progress,
    cursor: &mut Ticks,
    restore_settings: &mut bool,
) -> Result<(), MediaError> {
    let source_video = &group.video;
    let frames = &group.frames;
    let mut input = format::input(&group.input)?;
    let packaging = packaging_for(
        source_video.parameters.id(),
        &extradata_of(&source_video.parameters),
    );

    // Tracks line up by their order in each file, which the join check has confirmed.
    let mut audio_tracks: HashMap<usize, SourceAudioTrack> = HashMap::new();
    for stream in input.streams() {
        if stream.parameters().medium() == Type::Audio {
            let track = SourceAudioTrack {
                output_index: audio_tracks.len() + 1,
                time_base: stream.time_base(),
                finished_section: false,
            };
            audio_tracks.insert(stream.index(), track);
        }
    }
    let mut keep_streams: Vec<usize> = audio_tracks.keys().copied().collect();
    keep_streams.push(source_video.index);
    discard_other_streams(&mut input, &keep_streams);

    let positions: HashMap<i64, usize> = frames
        .frames()
        .iter()
        .enumerate()
        .map(|(position, frame)| (frame.pts, position))
        .collect();
    let durations: HashMap<i64, i64> = frames
        .frames()
        .iter()
        .map(|frame| (frame.pts, frame.duration))
        .collect();
    let time_base = to_time_base(source_video.time_base);
    let output_core_time_base = to_time_base(writer.output_time_base[0]);

    for section in &group.plan.sections {
        let read_first = section
            .pieces
            .iter()
            .map(|&p| piece_first(p))
            .min()
            .unwrap_or(0);
        let read_end = section
            .pieces
            .iter()
            .map(|&p| piece_end(p))
            .max()
            .unwrap_or(0);
        let section_end_seconds = f64::from(source_video.time_base) * section.pts_end as f64;

        // Starting a little early catches audio stored just before the keyframe.
        let start_dts = frames.frames()[read_first].dts;
        let seek_target = rescale(
            start_dts,
            source_video.time_base,
            Rational(1, ffi::AV_TIME_BASE),
        ) - SEEK_MARGIN_SECONDS * i64::from(ffi::AV_TIME_BASE);
        if seek_target > 0 {
            input.seek(seek_target, ..seek_target)?;
        } else {
            input.seek(0, ..0)?;
        }

        for track in audio_tracks.values_mut() {
            track.finished_section = false;
        }

        let mut section_writer = SectionWriter {
            source: source_video,
            packaging: &packaging,
            encoder_name: job.encoder_name,
            section_start: section.pts_start,
            output_start: cursor.to_pts(output_core_time_base),
            output_time_base: writer.output_time_base[0],
            run: None,
            restore_settings: *restore_settings,
            durations: &durations,
        };

        let mut piece_index = 0;
        let mut piece_started = false;
        let mut video_done = false;
        let mut seen = 0usize;

        for (stream, packet) in input.packets() {
            let stream_index = stream.index();
            seen += 1;
            if seen.is_multiple_of(64) && job.cancel.load(Ordering::Relaxed) {
                return Err(MediaError::Cancelled);
            }

            if stream_index == source_video.index {
                let Some(position) = packet.pts().and_then(|pts| positions.get(&pts).copied())
                else {
                    continue;
                };
                if position < read_first {
                    continue;
                }
                if position >= read_end {
                    video_done = true;
                }
                if !video_done {
                    while piece_index < section.pieces.len()
                        && position >= piece_end(section.pieces[piece_index])
                    {
                        section_writer.finish_run(writer)?;
                        piece_index += 1;
                        piece_started = false;
                    }
                    if let Some(&piece) = section.pieces.get(piece_index) {
                        if !piece_started {
                            section_writer.start_piece(piece)?;
                            piece_started = true;
                        }
                        match piece {
                            Piece::Copy { .. } => {
                                section_writer.write_copied(&packet, writer)?;
                            }
                            Piece::Encode { .. } => {
                                if let Some(run) = section_writer.run.as_mut() {
                                    run.decoder.send_packet(&packet)?;
                                }
                                section_writer.encode_decoded(writer)?;
                            }
                        }
                        progress.tick();
                    }
                }
            } else if let Some(track) = audio_tracks.get_mut(&stream_index) {
                write_audio_packet(
                    track,
                    &packet,
                    section,
                    source_video.time_base,
                    *cursor,
                    writer,
                )?;
            }

            if video_done {
                let packet_time_base = audio_tracks
                    .get(&stream_index)
                    .map_or(source_video.time_base, |track| track.time_base);
                let seconds = packet
                    .pts()
                    .map_or(0.0, |pts| f64::from(packet_time_base) * pts as f64);
                let all_audio_done = audio_tracks.values().all(|track| track.finished_section);
                if all_audio_done || seconds > section_end_seconds + AUDIO_READ_AHEAD_SECONDS {
                    break;
                }
            }
        }

        section_writer.finish_run(writer)?;
        *restore_settings = section_writer.restore_settings;
        let length = Ticks::from_pts(section.pts_end, time_base).0
            - Ticks::from_pts(section.pts_start, time_base).0;
        *cursor = Ticks(cursor.0 + length);
    }
    Ok(())
}

fn write_audio_packet(
    track: &mut SourceAudioTrack,
    packet: &Packet,
    section: &Section,
    video_time_base: Rational,
    cursor: Ticks,
    writer: &mut Writer,
) -> Result<(), MediaError> {
    let Some(pts) = packet.pts() else {
        return Ok(());
    };
    let start = rescale(section.pts_start, video_time_base, track.time_base);
    let end = rescale(section.pts_end, video_time_base, track.time_base);
    if pts >= end {
        track.finished_section = true;
        return Ok(());
    }
    let duration = packet.duration().max(1);
    if pts + duration <= start {
        return Ok(());
    }

    let output_time_base = writer.output_time_base[track.output_index];
    let output_start = cursor.to_pts(to_time_base(output_time_base));
    let output_duration = rescale(duration, track.time_base, output_time_base).max(1);
    let mut out_pts = rescale(pts - start, track.time_base, output_time_base) + output_start;
    if let Some(next_free) = writer.next_free[track.output_index] {
        // Audio packets rarely line up exactly with a cut, so neighbouring
        // sections can overlap by part of a packet.
        if out_pts + output_duration <= next_free {
            return Ok(());
        }
        out_pts = out_pts.max(next_free);
    }
    writer.next_free[track.output_index] = Some(out_pts + output_duration);

    let mut output = Packet::copy(packet.data().unwrap_or_default());
    output.set_flags(packet.flags());
    output.set_pts(Some(out_pts));
    output.set_dts(Some(out_pts));
    output.set_duration(output_duration);
    writer.write(&mut output, track.output_index)?;
    Ok(())
}

pub(crate) fn temporary_path(output: &Path) -> PathBuf {
    let extension = output.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    output.with_file_name(format!("{stem}.partial.{extension}"))
}

/// Works out what an export would do, without doing it.
pub fn plan_export(
    frames: &FrameIndex,
    ranges: &[TimeRange],
    time_base: TimeBase,
    mode: CutMode,
) -> CutPlan {
    let pts_ranges: Vec<(i64, i64)> = ranges
        .iter()
        .map(|range| (range.start.to_pts(time_base), range.end.to_pts(time_base)))
        .collect();
    frames.plan(&pts_ranges, mode)
}

fn load_source_video(path: &Path) -> Result<SourceVideo, MediaError> {
    let input = format::input(path)?;
    let stream = input
        .streams()
        .best(Type::Video)
        .ok_or(MediaError::NoVideoStream)?;
    let parameters = stream.parameters();
    // SAFETY: the parameters are live, and this reads one plain integer field.
    let declared = unsafe { (*parameters.as_ptr()).bit_rate };
    let overall = input.bit_rate();
    let rate = stream.avg_frame_rate();
    Ok(SourceVideo {
        index: stream.index(),
        time_base: stream.time_base(),
        frame_rate: if rate.denominator() == 0 || rate.numerator() == 0 {
            Rational(30, 1)
        } else {
            rate
        },
        // Encoders without a quality setting use this. Staying above the source
        // protects quality, and only short sections are ever encoded.
        bit_rate: (declared.max(overall).max(2_000_000) as usize).saturating_mul(3) / 2,
        parameters,
    })
}

/// Neighbouring parts of one file with no dead space between them.
#[derive(Clone)]
struct PartGroup {
    input: PathBuf,
    frames: Arc<FrameIndex>,
    ranges: Vec<TimeRange>,
    gap_before: Ticks,
}

/// Groups neighbouring parts of the same file, since each file is opened once.
fn group_parts(parts: &[ExportPart]) -> Vec<PartGroup> {
    let mut groups: Vec<PartGroup> = Vec::new();
    for part in parts {
        match groups.last_mut() {
            Some(group) if group.input == part.input && part.gap_before.0 <= 0 => {
                group.ranges.push(part.range);
            }
            _ => groups.push(PartGroup {
                input: part.input.clone(),
                frames: part.frames.clone(),
                ranges: vec![part.range],
                gap_before: part.gap_before,
            }),
        }
    }
    groups
}

/// Checks that every source can share one output, and returns why not if it can't.
fn check_joinable(parts: &[ExportPart]) -> Result<(), MediaError> {
    let mut first = None;
    for part in parts {
        let info = probe(&part.input)?;
        match &first {
            None => first = Some(info),
            Some(first) => {
                if let Some(problem) = join_problem(first, &info) {
                    return Err(MediaError::CannotJoin(problem));
                }
            }
        }
    }
    Ok(())
}

/// Exports the parts in order. The output only appears once it is complete. If
/// frame-exact cuts need an encoder a video has none for, the export falls back to
/// keyframe cuts, and the report says so.
pub fn export(
    request: &ExportRequest,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<ExportReport, MediaError> {
    crate::init()?;
    let started = Instant::now();
    if request.parts.is_empty() {
        return Err(MediaError::NothingToExport);
    }
    if request
        .parts
        .iter()
        .any(|part| same_file(&part.input, &request.output))
    {
        return Err(MediaError::SameInputAndOutput);
    }
    check_joinable(&request.parts)?;

    let plan_groups = |mode: CutMode, sources: Vec<PartGroup>| {
        sources
            .into_iter()
            .map(|part| {
                let video = load_source_video(&part.input)?;
                let plan = plan_export(
                    &part.frames,
                    &part.ranges,
                    to_time_base(video.time_base),
                    mode,
                );
                Ok(SourceGroup {
                    input: part.input,
                    frames: part.frames,
                    gap_before: part.gap_before,
                    video,
                    plan,
                })
            })
            .collect::<Result<Vec<SourceGroup>, MediaError>>()
    };

    let mut mode_used = request.mode;
    let sources = group_parts(&request.parts);
    let mut groups = plan_groups(mode_used, sources.clone())?;
    let mut encoder_name = None;
    if let Some(needing) = groups
        .iter()
        .find(|group| group.plan.encoded_frames > 0 || group.black_frames() > 0)
    {
        encoder_name = choose_encoder(needing.video.parameters.id(), &needing.video);
        if encoder_name.is_none() {
            if groups.iter().any(|group| group.black_frames() > 0) {
                return Err(MediaError::GapNeedsEncoder);
            }
            mode_used = CutMode::Keyframe;
            groups = plan_groups(mode_used, sources)?;
        }
    }
    if groups.iter().all(|group| group.plan.sections.is_empty()) {
        return Err(MediaError::NothingToExport);
    }

    let temporary = temporary_path(&request.output);
    let result = run_export(&ExportJob {
        request,
        temporary: &temporary,
        groups: &groups,
        encoder_name: encoder_name.as_deref(),
        cancel,
        progress,
    });
    let audio_tracks = match result {
        Ok(count) => count,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
    };
    std::fs::rename(&temporary, &request.output).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })?;
    progress(1.0);

    let total = |count: fn(&CutPlan) -> usize| groups.iter().map(|group| count(&group.plan)).sum();
    Ok(ExportReport {
        mode_requested: request.mode,
        mode_used,
        sections: total(|plan| plan.sections.len()),
        copied_frames: total(|plan| plan.copied_frames),
        encoded_frames: total(|plan| plan.encoded_frames),
        black_frames: groups.iter().map(SourceGroup::black_frames).sum(),
        encoder: encoder_name,
        audio_tracks,
        elapsed: started.elapsed(),
    })
}

/// Whether dead space can be written for a video with this codec.
pub fn can_write_gaps(codec: &str) -> bool {
    encoder::find_by_name(codec)
        .or_else(|| decoder::find_by_name(codec))
        .is_some_and(|found| !encoder_candidates(found.id()).is_empty())
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescaling_rounds_to_the_nearest_value() {
        assert_eq!(rescale(1, Rational(1, 1000), Rational(1, 48_000)), 48);
        assert_eq!(rescale(-1, Rational(1, 1000), Rational(1, 48_000)), -48);
        assert_eq!(rescale(1, Rational(1, 48_000), Rational(1, 1000)), 0);
        assert_eq!(rescale(24, Rational(1, 48_000), Rational(1, 1000)), 1);
    }

    #[test]
    fn the_partial_file_keeps_its_extension_so_the_container_is_right() {
        assert_eq!(
            temporary_path(Path::new("C:/videos/clip.mkv")),
            Path::new("C:/videos/clip.partial.mkv")
        );
    }

    #[test]
    fn the_stored_order_never_goes_backwards() {
        let mut timeline = VideoTimeline::new(0);

        assert_eq!(timeline.place(10, 0), (10, 0));
        assert_eq!(timeline.place(20, 0), (20, 1));
        assert_eq!(timeline.place(30, -5), (30, 2));
    }

    #[test]
    fn a_display_time_never_comes_before_its_stored_time() {
        let mut timeline = VideoTimeline::new(0);

        timeline.place(10, 50);

        assert_eq!(timeline.place(20, 0), (51, 51));
    }

    #[test]
    fn hardware_encoders_are_tried_before_software() {
        let candidates = encoder_candidates(Id::H264);

        assert_eq!(candidates[0], "h264_nvenc");
        assert!(
            candidates.iter().position(|name| name == "libopenh264")
                > candidates.iter().position(|name| name == "h264_nvenc")
        );
    }

    #[test]
    fn codecs_not_proven_to_join_cleanly_have_no_encoders() {
        assert!(encoder_candidates(Id::MPEG4).is_empty());
        assert!(encoder_candidates(Id::VP9).is_empty());
    }

    #[test]
    fn dead_space_rounds_to_whole_frames() {
        let thirty = Rational(30, 1);

        assert_eq!(black_frame_count(Ticks::from_seconds(1.0), thirty), 30);
        assert_eq!(black_frame_count(Ticks::from_seconds(0.05), thirty), 2);
    }

    #[test]
    fn a_sliver_of_dead_space_needs_no_frames() {
        assert_eq!(
            black_frame_count(Ticks::from_seconds(0.01), Rational(30, 1)),
            0
        );
        assert_eq!(black_frame_count(Ticks(0), Rational(30, 1)), 0);
    }

    #[test]
    fn dead_space_is_only_written_for_codecs_that_join_cleanly() {
        crate::init().expect("FFmpeg starts");

        assert!(can_write_gaps("h264"));
        assert!(can_write_gaps("hevc"));
        assert!(!can_write_gaps("mpeg4"));
    }

    #[test]
    fn settings_are_restored_in_the_packaging_of_the_stream() {
        let avcc = [
            1, 0x64, 0, 0x28, 0xff, 0xe1, 0, 2, 0x67, 0xaa, 1, 0, 1, 0x68,
        ];

        let packaging = packaging_for(Id::H264, &avcc);

        assert_eq!(packaging.length_size, Some(4));
        assert_eq!(
            packaging.source_settings,
            [0, 0, 0, 2, 0x67, 0xaa, 0, 0, 0, 1, 0x68]
        );
    }

    #[test]
    fn other_codecs_restore_their_header_unchanged() {
        let packaging = packaging_for(Id::MPEG4, &[9, 8, 7]);

        assert_eq!(packaging.length_size, None);
        assert_eq!(packaging.source_settings, [9, 8, 7]);
    }
}
