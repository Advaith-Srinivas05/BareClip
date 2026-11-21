//! Everything a file says about itself, for the player's info panel, and writing a copy of a
//! file with new tags. This is apart from `probe`, which reads only what playing needs and has
//! to stay fast.

use std::{
    ffi::CStr,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use bareclip_core::time::{Ticks, TimeBase};
use ffmpeg_next::{self as ffmpeg, Packet, codec, encoder, ffi, format, media::Type};

use crate::error::MediaError;

/// One stream of a file, described.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamDetails {
    Picture {
        codec: String,
        profile: Option<String>,
        width: u32,
        height: u32,
        /// Frames a second on average.
        frame_rate: f64,
        /// Whether frames come at uneven times, as they do in many screen recordings.
        variable_rate: bool,
        /// Bits per colour value, such as 8 or 10.
        bit_depth: Option<u32>,
        /// The colour standard, such as BT.709.
        colours: Option<String>,
        hdr: Option<&'static str>,
        bit_rate: Option<u64>,
    },
    Sound {
        codec: String,
        profile: Option<String>,
        channels: u16,
        /// How the channels are laid out, such as stereo or 5.1.
        layout: Option<String>,
        sample_rate: u32,
        bit_rate: Option<u64>,
        name: Option<String>,
        language: Option<String>,
    },
}

/// Everything shown about a file.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaDetails {
    /// The container's long name, such as "QuickTime / MOV".
    pub container: String,
    pub duration: Ticks,
    pub size_bytes: u64,
    /// The whole file's bits a second.
    pub bit_rate: Option<u64>,
    /// Picture and sound streams, in the file's order, leaving out a song's cover.
    pub streams: Vec<StreamDetails>,
    pub subtitles: usize,
    pub chapters: usize,
    /// Every tag on the file itself, such as title, artist, encoder and creation time.
    pub tags: Vec<(String, String)>,
}

impl MediaDetails {
    /// A tag's value, ignoring case in its name.
    pub fn tag(&self, name: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

fn text_of(pointer: *const std::os::raw::c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: FFmpeg's name functions give static, nul-terminated strings or null.
    Some(
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn rate_of(rational: ffmpeg::Rational) -> f64 {
    if rational.denominator() == 0 {
        0.0
    } else {
        f64::from(rational.numerator()) / f64::from(rational.denominator())
    }
}

/// Reads everything the info panel shows. It reads only the file's header, so it is quick, but
/// it runs off the interface's thread anyway.
pub fn read_details(path: &Path) -> Result<MediaDetails, MediaError> {
    crate::init()?;
    let size_bytes = std::fs::metadata(path)?.len();
    let input = format::input(path)?;
    let container = input.format().description().to_owned();
    let duration = Ticks::from_pts(
        input.duration().max(0),
        TimeBase {
            num: 1,
            den: ffi::AV_TIME_BASE,
        },
    );
    // SAFETY: the context is live for this whole function.
    let bit_rate = unsafe { (*input.as_ptr()).bit_rate };
    let mut streams = Vec::new();
    let mut subtitles = 0;
    for stream in input.streams() {
        let parameters = stream.parameters();
        // SAFETY: the parameters belong to a live stream, and only plain fields are read.
        let raw = unsafe { &*parameters.as_ptr() };
        let codec = parameters.id().name().to_owned();
        let profile = text_of(unsafe { ffi::avcodec_profile_name(raw.codec_id, raw.profile) });
        let stream_bit_rate = u64::try_from(raw.bit_rate).ok().filter(|rate| *rate > 0);
        let metadata = stream.metadata();
        match parameters.medium() {
            Type::Video
                if !stream
                    .disposition()
                    .contains(format::stream::Disposition::ATTACHED_PIC) =>
            {
                let average = rate_of(stream.avg_frame_rate());
                let base = rate_of(stream.rate());
                // SAFETY: the format is one of FFmpeg's own, or none, which gives null.
                let description = unsafe {
                    ffi::av_pix_fmt_desc_get(std::mem::transmute::<i32, ffi::AVPixelFormat>(
                        raw.format,
                    ))
                };
                let bit_depth = if raw.bits_per_raw_sample > 0 {
                    Some(raw.bits_per_raw_sample as u32)
                } else if description.is_null() {
                    None
                } else {
                    // SAFETY: a non-null description is a static table entry.
                    Some(unsafe { (*description).comp[0].depth } as u32)
                };
                let transfer = raw.color_trc;
                let hdr = match transfer {
                    ffi::AVColorTransferCharacteristic::AVCOL_TRC_SMPTE2084 => Some("HDR10 (PQ)"),
                    ffi::AVColorTransferCharacteristic::AVCOL_TRC_ARIB_STD_B67 => Some("HLG"),
                    _ => None,
                };
                let colours = (raw.color_space != ffi::AVColorSpace::AVCOL_SPC_UNSPECIFIED)
                    .then(|| text_of(unsafe { ffi::av_color_space_name(raw.color_space) }))
                    .flatten()
                    .map(|name| name.to_uppercase().replace("BT", "BT."));
                streams.push(StreamDetails::Picture {
                    codec,
                    profile,
                    width: raw.width.max(0) as u32,
                    height: raw.height.max(0) as u32,
                    frame_rate: average,
                    variable_rate: average > 0.0 && base > 0.0 && (average - base).abs() > 0.01,
                    bit_depth,
                    colours,
                    hdr,
                    bit_rate: stream_bit_rate,
                });
            }
            Type::Audio => {
                let mut described = [0_u8; 64];
                // SAFETY: the buffer's size is passed, and FFmpeg ends what it writes with a nul.
                let written = unsafe {
                    ffi::av_channel_layout_describe(
                        &raw.ch_layout,
                        described.as_mut_ptr().cast(),
                        described.len(),
                    )
                };
                let layout = (written > 0)
                    .then(|| text_of(described.as_ptr().cast()))
                    .flatten();
                streams.push(StreamDetails::Sound {
                    codec,
                    profile,
                    channels: raw.ch_layout.nb_channels.max(0) as u16,
                    layout,
                    sample_rate: raw.sample_rate.max(0) as u32,
                    bit_rate: stream_bit_rate,
                    name: metadata
                        .get("title")
                        .or_else(|| metadata.get("name"))
                        .map(str::to_owned),
                    language: metadata
                        .get("language")
                        .filter(|language| *language != "und")
                        .map(str::to_owned),
                });
            }
            Type::Subtitle => subtitles += 1,
            _ => {}
        }
    }
    let tags = input
        .metadata()
        .iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    Ok(MediaDetails {
        container,
        duration,
        size_bytes,
        bit_rate: u64::try_from(bit_rate).ok().filter(|rate| *rate > 0),
        streams,
        subtitles,
        chapters: input.chapters().count(),
        tags,
    })
}

/// Which tags BareClip can write into a kind of file, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagSupport {
    pub artist: bool,
    /// The moment the file says it was made, `creation_time`. Only MP4, MOV and Matroska have a
    /// place for it that players read.
    pub creation_time: bool,
}

pub fn tag_support(path: &Path) -> TagSupport {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        "mp4" | "m4v" | "mov" | "m4a" | "mkv" | "mka" | "webm" => TagSupport {
            artist: true,
            creation_time: true,
        },
        "mp3" | "flac" | "ogg" | "opus" => TagSupport {
            artist: true,
            creation_time: false,
        },
        _ => TagSupport {
            artist: false,
            creation_time: false,
        },
    }
}

/// New values for tags. `None` leaves a tag as it is, and an empty value takes it away.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagChanges {
    pub artist: Option<String>,
    /// As FFmpeg writes it, such as `2025-09-14T01:35:00.000000Z`.
    pub creation_time: Option<String>,
}

/// Writes a copy of a file with changed tags, copying every stream, the chapters and the
/// other tags exactly, with nothing re-encoded. `progress` gets the share of the file copied.
/// A cancelled or failed copy leaves no file behind.
pub fn write_with_tags(
    source: &Path,
    copy: &Path,
    changes: &TagChanges,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<(), MediaError> {
    crate::init()?;
    let result = copy_with_tags(source, copy, changes, cancel, &mut progress);
    if result.is_err() {
        let _ = std::fs::remove_file(copy);
    }
    result
}

fn copy_with_tags(
    source: &Path,
    copy: &Path,
    changes: &TagChanges,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), MediaError> {
    let size = std::fs::metadata(source)?.len().max(1);
    let mut input = format::input(source)?;
    let muxer = guess_muxer(source)?;
    let mut output = format::output_as(copy, &muxer)?;

    let mut places = Vec::new();
    for stream in input.streams() {
        // SAFETY: the stream is live, and `discard` is a plain field.
        let hidden = unsafe { (*stream.as_ptr()).discard } == ffi::AVDiscard::AVDISCARD_ALL;
        if hidden {
            // A track FFmpeg keeps to itself, such as an MP4's chapter text, which the muxer
            // writes again from the chapters.
            places.push(None);
            continue;
        }
        let medium = stream.parameters().medium();
        let mut added = output.add_stream(encoder::find(codec::Id::None))?;
        added.set_parameters(stream.parameters());
        added.set_time_base(stream.time_base());
        added.set_avg_frame_rate(stream.avg_frame_rate());
        added.set_rate(stream.rate());
        added.set_metadata(stream.metadata().to_owned());
        // SAFETY: both streams are live, and these are plain fields the muxer reads.
        unsafe {
            (*added.as_mut_ptr()).disposition = (*stream.as_ptr()).disposition;
            if !matches!(medium, Type::Video | Type::Audio) {
                // Data and subtitles take whatever tag the muxer gives them.
                (*added.parameters().as_mut_ptr()).codec_tag = 0;
            }
        }
        places.push(Some((added.index(), stream.time_base())));
    }
    for chapter in input.chapters() {
        let title = chapter.metadata().get("title").unwrap_or("").to_owned();
        output.add_chapter(
            chapter.id(),
            chapter.time_base(),
            chapter.start(),
            chapter.end(),
            title,
        )?;
    }
    let mut metadata = input.metadata().to_owned();
    if let Some(artist) = &changes.artist {
        set_or_remove(&mut metadata, "artist", artist);
    }
    if let Some(time) = &changes.creation_time {
        set_or_remove(&mut metadata, "creation_time", time);
    }
    output.set_metadata(metadata);
    output.write_header()?;
    let output_bases: Vec<ffmpeg::Rational> =
        output.streams().map(|stream| stream.time_base()).collect();

    let mut packet = Packet::empty();
    let mut last_report = 0.0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        match packet.read(&mut input) {
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(error.into()),
        }
        let index = packet.stream();
        let Some(&Some((place, from))) = places.get(index) else {
            continue;
        };
        let read_to = packet.position();
        packet.set_stream(place);
        packet.rescale_ts(from, output_bases[place]);
        packet.set_position(-1);
        packet.write_interleaved(&mut output)?;
        if read_to > 0 {
            let share = (read_to as f64 / size as f64).min(1.0) as f32;
            if share - last_report >= 0.01 {
                last_report = share;
                progress(share);
            }
        }
    }
    output.write_trailer()?;
    progress(1.0);
    Ok(())
}

fn set_or_remove(metadata: &mut ffmpeg::Dictionary, key: &str, value: &str) {
    let mut kept = ffmpeg::Dictionary::new();
    for (name, existing) in metadata.iter() {
        if !name.eq_ignore_ascii_case(key) {
            kept.set(name, existing);
        }
    }
    if !value.trim().is_empty() {
        kept.set(key, value.trim());
    }
    *metadata = kept;
}

/// The muxer for a file's own kind, going by its extension, so a copy is written the same way
/// whatever it is named.
fn guess_muxer(path: &Path) -> Result<String, MediaError> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let name = match extension.as_str() {
        "mp4" | "m4v" => "mp4",
        "mov" => "mov",
        "m4a" => "ipod",
        "mkv" | "mka" => "matroska",
        "webm" => "webm",
        "mp3" => "mp3",
        "flac" => "flac",
        "ogg" => "ogg",
        "opus" => "opus",
        _ => return Err(ffmpeg::Error::MuxerNotFound.into()),
    };
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests;
