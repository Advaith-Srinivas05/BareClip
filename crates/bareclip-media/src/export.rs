//! Writes the kept sections of a video to a new file.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use bareclip_core::{
    cut::TimeRange,
    splice::{CutMode, CutPlan, FrameIndex, Piece, Section},
    time::TimeBase,
};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{
    Dictionary, Packet, Rational, codec, codec::Id, decoder, encoder, ffi, format, frame,
    media::Type, picture,
};

use crate::{
    error::MediaError,
    nal,
    probe::{discard_other_streams, to_time_base},
};

const SEEK_MARGIN_SECONDS: i64 = 2;
/// How far past a section to keep reading while waiting for its last audio.
const AUDIO_READ_AHEAD_SECONDS: f64 = 10.0;

#[derive(Clone, Debug)]
pub struct ExportRequest {
    pub input: PathBuf,
    pub output: PathBuf,
    pub ranges: Vec<TimeRange>,
    pub mode: CutMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportReport {
    pub mode_requested: CutMode,
    pub mode_used: CutMode,
    pub sections: usize,
    pub copied_frames: usize,
    pub encoded_frames: usize,
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
) -> Result<encoder::Video, ffmpeg::Error> {
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
    video.open_with(options)
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
    fn new(frames: &FrameIndex) -> Self {
        Self {
            delay: frames
                .frames()
                .iter()
                .map(|frame| frame.pts - frame.dts)
                .max()
                .unwrap_or(0)
                .max(0),
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

struct AudioTrack {
    output_index: usize,
    time_base: Rational,
    /// Where the previous packet written to this track ended.
    next_free: Option<i64>,
    finished_section: bool,
}

/// The muxer and everything needed to place packets into it.
struct Writer {
    output: format::context::Output,
    output_time_base: Vec<Rational>,
    video_timeline: VideoTimeline,
}

impl Writer {
    fn write(
        &mut self,
        packet: &mut Packet,
        output_index: usize,
        from: Rational,
    ) -> Result<(), ffmpeg::Error> {
        packet.rescale_ts(from, self.output_time_base[output_index]);
        packet.set_position(-1);
        packet.set_stream(output_index);
        packet.write_interleaved(&mut self.output)
    }
}

/// A re-encoded piece in progress.
struct EncodeRun {
    decoder: decoder::Video,
    encoder: encoder::Video,
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
    /// Added to source display times to get output display times.
    offset: i64,
    run: Option<EncodeRun>,
    /// A re-encoded part put different settings into the stream, so the next
    /// copied frame must bring back the source's settings.
    restore_settings: bool,
    durations: &'a HashMap<i64, i64>,
}

impl SectionWriter<'_> {
    fn write_encoded(&mut self, writer: &mut Writer) -> Result<(), MediaError> {
        let Some(run) = self.run.as_mut() else {
            return Ok(());
        };
        let mut encoded = Packet::empty();
        while run.encoder.receive_packet(&mut encoded).is_ok() {
            let Some(pts) = encoded.pts() else {
                continue;
            };
            let out_pts = pts + self.offset;
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
            packet.set_duration(self.durations.get(&pts).copied().unwrap_or(1));
            writer.write(&mut packet, 0, self.source.time_base)?;
        }
        Ok(())
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
            .place(pts + self.offset, dts + self.offset);

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
            self.durations
                .get(&pts)
                .copied()
                .unwrap_or_else(|| packet.duration()),
        );
        writer.write(&mut output, 0, self.source.time_base)?;
        Ok(())
    }
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

/// Everything one run of the export needs.
struct ExportJob<'a> {
    request: &'a ExportRequest,
    temporary: &'a Path,
    frames: &'a FrameIndex,
    plan: &'a CutPlan,
    source_video: &'a SourceVideo,
    encoder_name: Option<&'a str>,
    cancel: &'a AtomicBool,
    progress: &'a dyn Fn(f32),
}

fn run_export(job: &ExportJob) -> Result<usize, MediaError> {
    let ExportJob {
        request,
        temporary,
        frames,
        plan,
        source_video,
        encoder_name,
        cancel,
        progress,
    } = *job;
    let mut input = format::input(&request.input)?;
    let mut output = format::output(temporary)?;
    let same_container = request.input.extension() == request.output.extension();
    let encoded_any = plan.encoded_frames > 0;

    let packaging = packaging_for(
        source_video.parameters.id(),
        &extradata_of(&source_video.parameters),
    );

    // The video always becomes output stream 0.
    {
        let stream = input
            .stream(source_video.index)
            .ok_or(MediaError::NoVideoStream)?;
        let mut added = output.add_stream(encoder::find(Id::None))?;
        added.set_parameters(stream.parameters());
        added.set_time_base(stream.time_base());
        added.set_avg_frame_rate(stream.avg_frame_rate());
        copy_stream_identity(&stream, &mut added);
        // A file header type that promises settings live only in the header would
        // be untrue once re-encoded parts carry their own, so the muxer picks again.
        if !same_container || encoded_any {
            // SAFETY: the parameters belong to the stream just added.
            unsafe {
                (*added.parameters().as_mut_ptr()).codec_tag = 0;
            }
        }
    }

    let mut audio_tracks: HashMap<usize, AudioTrack> = HashMap::new();
    for stream in input.streams() {
        if stream.parameters().medium() != Type::Audio {
            continue;
        }
        let mut added = output.add_stream(encoder::find(Id::None))?;
        added.set_parameters(stream.parameters());
        added.set_time_base(stream.time_base());
        copy_stream_identity(&stream, &mut added);
        if !same_container {
            // SAFETY: the parameters belong to the stream just added.
            unsafe {
                (*added.parameters().as_mut_ptr()).codec_tag = 0;
            }
        }
        let output_index = added.index();
        audio_tracks.insert(
            stream.index(),
            AudioTrack {
                output_index,
                time_base: stream.time_base(),
                next_free: None,
                finished_section: false,
            },
        );
    }
    let audio_count = audio_tracks.len();

    output.set_metadata(input.metadata().to_owned());
    output.write_header()?;

    let mut keep_streams: Vec<usize> = audio_tracks.keys().copied().collect();
    keep_streams.push(source_video.index);
    discard_other_streams(&mut input, &keep_streams);

    let output_time_base = output.streams().map(|stream| stream.time_base()).collect();
    let mut writer = Writer {
        output,
        output_time_base,
        video_timeline: VideoTimeline::new(frames),
    };

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

    let total_frames: usize = plan
        .sections
        .iter()
        .flat_map(|section| &section.pieces)
        .map(|&piece| piece_end(piece) - piece_first(piece))
        .sum();
    let mut progress = Progress {
        report: progress,
        total: total_frames,
        done: 0,
    };

    let mut timeline_cursor = 0;
    let mut restore_settings = false;
    for section in &plan.sections {
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

        let offset = timeline_cursor - section.pts_start;
        let mut section_writer = SectionWriter {
            source: source_video,
            packaging: &packaging,
            encoder_name,
            offset,
            run: None,
            restore_settings,
            durations: &durations,
        };

        let mut piece_index = 0;
        let mut piece_started = false;
        let mut video_done = false;
        let mut seen = 0usize;

        for (stream, packet) in input.packets() {
            let stream_index = stream.index();
            seen += 1;
            if seen.is_multiple_of(64) && cancel.load(Ordering::Relaxed) {
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
                        section_writer.finish_run(&mut writer)?;
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
                                section_writer.write_copied(&packet, &mut writer)?;
                            }
                            Piece::Encode { .. } => {
                                if let Some(run) = section_writer.run.as_mut() {
                                    run.decoder.send_packet(&packet)?;
                                }
                                section_writer.encode_decoded(&mut writer)?;
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
                    timeline_cursor,
                    &mut writer,
                )?;
            }

            if video_done {
                let time_base = input_time_base(&audio_tracks, stream_index, source_video);
                let seconds = packet
                    .pts()
                    .map_or(0.0, |pts| f64::from(time_base) * pts as f64);
                let all_audio_done = audio_tracks.values().all(|track| track.finished_section);
                if all_audio_done || seconds > section_end_seconds + AUDIO_READ_AHEAD_SECONDS {
                    break;
                }
            }
        }

        section_writer.finish_run(&mut writer)?;
        restore_settings = section_writer.restore_settings;
        timeline_cursor += section.pts_end - section.pts_start;
    }

    writer.output.write_trailer()?;
    Ok(audio_count)
}

fn input_time_base(
    tracks: &HashMap<usize, AudioTrack>,
    stream_index: usize,
    video: &SourceVideo,
) -> Rational {
    tracks
        .get(&stream_index)
        .map_or(video.time_base, |track| track.time_base)
}

fn write_audio_packet(
    track: &mut AudioTrack,
    packet: &Packet,
    section: &Section,
    video_time_base: Rational,
    timeline_cursor: i64,
    writer: &mut Writer,
) -> Result<(), MediaError> {
    let Some(pts) = packet.pts() else {
        return Ok(());
    };
    let start = rescale(section.pts_start, video_time_base, track.time_base);
    let end = rescale(section.pts_end, video_time_base, track.time_base);
    let cursor = rescale(timeline_cursor, video_time_base, track.time_base);
    if pts >= end {
        track.finished_section = true;
        return Ok(());
    }
    let duration = packet.duration().max(1);
    if pts + duration <= start {
        return Ok(());
    }

    let mut out_pts = pts - start + cursor;
    if let Some(next_free) = track.next_free {
        // Audio packets rarely line up exactly with a cut, so neighbouring
        // sections can overlap by part of a packet.
        if out_pts + duration <= next_free {
            return Ok(());
        }
        out_pts = out_pts.max(next_free);
    }
    track.next_free = Some(out_pts + duration);

    let mut output = Packet::copy(packet.data().unwrap_or_default());
    output.set_flags(packet.flags());
    output.set_pts(Some(out_pts));
    output.set_dts(Some(out_pts));
    output.set_duration(duration);
    writer.write(&mut output, track.output_index, track.time_base)?;
    Ok(())
}

fn temporary_path(output: &Path) -> PathBuf {
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

/// Exports the kept sections. The output only appears once it is complete. If
/// frame-exact cuts need an encoder this video has none for, the export falls back
/// to keyframe cuts, and the report says so.
pub fn export(
    request: &ExportRequest,
    frames: &FrameIndex,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<ExportReport, MediaError> {
    crate::init()?;
    let started = Instant::now();
    if request.ranges.is_empty() {
        return Err(MediaError::NothingToExport);
    }
    if same_file(&request.input, &request.output) {
        return Err(MediaError::SameInputAndOutput);
    }

    let source_video = {
        let input = format::input(&request.input)?;
        let stream = input
            .streams()
            .best(Type::Video)
            .ok_or(MediaError::NoVideoStream)?;
        let parameters = stream.parameters();
        // SAFETY: the parameters are live, and this reads one plain integer field.
        let declared = unsafe { (*parameters.as_ptr()).bit_rate };
        let overall = input.bit_rate();
        let rate = stream.avg_frame_rate();
        SourceVideo {
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
        }
    };
    let time_base = to_time_base(source_video.time_base);

    let mut mode_used = request.mode;
    let mut plan = plan_export(frames, &request.ranges, time_base, mode_used);
    let mut encoder_name = None;
    if plan.encoded_frames > 0 {
        encoder_name = choose_encoder(source_video.parameters.id(), &source_video);
        if encoder_name.is_none() {
            mode_used = CutMode::Keyframe;
            plan = plan_export(frames, &request.ranges, time_base, mode_used);
        }
    }
    if plan.sections.is_empty() {
        return Err(MediaError::NothingToExport);
    }

    let temporary = temporary_path(&request.output);
    let result = run_export(&ExportJob {
        request,
        temporary: &temporary,
        frames,
        plan: &plan,
        source_video: &source_video,
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

    Ok(ExportReport {
        mode_requested: request.mode,
        mode_used,
        sections: plan.sections.len(),
        copied_frames: plan.copied_frames,
        encoded_frames: plan.encoded_frames,
        encoder: encoder_name,
        audio_tracks,
        elapsed: started.elapsed(),
    })
}

fn same_file(a: &Path, b: &Path) -> bool {
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
        let frames = FrameIndex::new(Vec::new());
        let mut timeline = VideoTimeline::new(&frames);

        assert_eq!(timeline.place(10, 0), (10, 0));
        assert_eq!(timeline.place(20, 0), (20, 1));
        assert_eq!(timeline.place(30, -5), (30, 2));
    }

    #[test]
    fn a_display_time_never_comes_before_its_stored_time() {
        let frames = FrameIndex::new(Vec::new());
        let mut timeline = VideoTimeline::new(&frames);

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
