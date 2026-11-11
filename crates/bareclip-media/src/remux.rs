//! Puts new sound with a video that was copied, so changing the sound doesn't mean redrawing the
//! picture.

use std::path::{Path, PathBuf};

use ffmpeg_next::{self as ffmpeg, Packet, Rational, codec, encoder, format, media::Type};

use crate::{
    MediaError,
    export::temporary_path,
    full::{SOUND_CHANNELS, SoundLabel, SoundOutput},
};

/// Writes stereo sound tracks to a file of their own.
pub struct SoundFile {
    output: format::context::Output,
    tracks: Vec<SoundOutput>,
    time_bases: Vec<Rational>,
}

impl SoundFile {
    pub fn create(path: &Path, labels: &[SoundLabel]) -> Result<Self, MediaError> {
        Self::open(path, labels, false)
    }

    /// A file that can be read at any moment while it is written, as a recording needs, so a
    /// crash keeps everything up to about the last second. It is an MP4 written in pieces of a
    /// second each, which players read like any other.
    pub fn create_growing(path: &Path, labels: &[SoundLabel]) -> Result<Self, MediaError> {
        Self::open(path, labels, true)
    }

    fn open(path: &Path, labels: &[SoundLabel], growing: bool) -> Result<Self, MediaError> {
        crate::init()?;
        let mut output = format::output(path)?;
        let tracks = labels
            .iter()
            .enumerate()
            .map(|(index, label)| SoundOutput::add_to(&mut output, index, label))
            .collect::<Result<Vec<_>, MediaError>>()?;
        if growing {
            output.write_header_with(crate::full::growing_options())?;
        } else {
            output.write_header()?;
        }
        let time_bases = (0..tracks.len())
            .map(|index| {
                output
                    .stream(index)
                    .map_or(Rational(1, 1), |stream| stream.time_base())
            })
            .collect();
        Ok(Self {
            output,
            tracks,
            time_bases,
        })
    }

    /// Adds stereo samples to the end of a sound track.
    pub fn write(&mut self, track: usize, samples: &[f32]) -> Result<(), MediaError> {
        let sound = &mut self.tracks[track];
        sound.waiting.extend_from_slice(samples);
        while sound.waiting.len() >= sound.block_frames * SOUND_CHANNELS {
            sound.send_block(sound.block_frames)?;
            sound.drain(self.time_bases[track], &mut self.output)?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), MediaError> {
        for (track, sound) in self.tracks.iter_mut().enumerate() {
            let left = sound.waiting.len() / SOUND_CHANNELS;
            if left > 0 {
                sound.send_block(left)?;
            }
            sound.encoder.send_eof()?;
            sound.drain(self.time_bases[track], &mut self.output)?;
        }
        self.output.write_trailer()?;
        Ok(())
    }
}

/// The next packet to come from a file, ready to be put in order.
struct Source {
    input: format::context::Input,
    /// Where each of its streams went in the output, and the time bases either side.
    streams: Vec<Option<(usize, Rational, Rational)>>,
    next: Option<Packet>,
}

impl Source {
    fn read_next(&mut self) -> Result<(), MediaError> {
        let mut packet = Packet::empty();
        self.next = match packet.read(&mut self.input) {
            Ok(()) => Some(packet),
            Err(ffmpeg::Error::Eof) => None,
            Err(error) => return Err(error.into()),
        };
        Ok(())
    }

    /// When the next packet is due, in seconds. Files can't be compared in their own time bases.
    fn next_time(&self) -> Option<f64> {
        let packet = self.next.as_ref()?;
        let (_, from, _) = self.streams.get(packet.stream()).copied().flatten()?;
        let time = packet.dts().or(packet.pts()).unwrap_or(0);
        Some(time as f64 * f64::from(from))
    }
}

fn add_streams(
    input: &format::context::Input,
    output: &mut format::context::Output,
    wanted: Type,
    clear_tag: bool,
) -> Result<Vec<Option<usize>>, MediaError> {
    let mut placed = Vec::new();
    for stream in input.streams() {
        if stream.parameters().medium() != wanted {
            placed.push(None);
            continue;
        }
        let mut added = output.add_stream(encoder::find(codec::Id::None))?;
        added.set_parameters(stream.parameters());
        added.set_time_base(stream.time_base());
        if wanted == Type::Video {
            added.set_avg_frame_rate(stream.avg_frame_rate());
        }
        added.set_metadata(stream.metadata().to_owned());
        if clear_tag {
            // SAFETY: the parameters belong to this stream, and `codec_tag` is a plain field the
            // muxer reads when writing the header.
            unsafe {
                (*added.parameters().as_mut_ptr()).codec_tag = 0;
            }
        }
        placed.push(Some(added.index()));
    }
    Ok(placed)
}

/// Makes one file from the picture of one and the sound of another, copying both. Nothing appears
/// at the real path until it is finished.
pub fn join_picture_and_sound(
    picture: &Path,
    sound: &Path,
    output_path: &Path,
) -> Result<(), MediaError> {
    crate::init()?;
    let temporary: PathBuf = temporary_path(output_path);
    let result = write_joined(picture, sound, &temporary);
    match result {
        Ok(()) => std::fs::rename(&temporary, output_path).map_err(MediaError::from),
        Err(error) => Err(error),
    }
    .inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

fn write_joined(picture: &Path, sound: &Path, path: &Path) -> Result<(), MediaError> {
    let picture_input = format::input(picture)?;
    let sound_input = format::input(sound)?;
    let mut output = format::output(path)?;
    let same_container = picture.extension() == path.extension();
    let picture_streams = add_streams(&picture_input, &mut output, Type::Video, !same_container)?;
    let sound_streams = add_streams(&sound_input, &mut output, Type::Audio, true)?;
    output.set_metadata(picture_input.metadata().to_owned());
    output.write_header()?;

    let mut sources = [picture_input, sound_input]
        .into_iter()
        .zip([picture_streams, sound_streams])
        .map(|(input, placed)| {
            let streams = placed
                .iter()
                .enumerate()
                .map(|(index, place)| {
                    let place = (*place)?;
                    let from = input.stream(index)?.time_base();
                    let to = output.stream(place)?.time_base();
                    Some((place, from, to))
                })
                .collect();
            let mut source = Source {
                input,
                streams,
                next: None,
            };
            source.read_next().map(|()| source)
        })
        .collect::<Result<Vec<_>, MediaError>>()?;

    loop {
        // Whichever is due first goes next, so the two stay woven together in the file.
        let earliest = sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| Some((index, source.next_time()?)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(index, _)| index);
        let Some(earliest) = earliest else {
            // Anything left has no stream to go to.
            if sources.iter().all(|source| source.next.is_none()) {
                break;
            }
            for source in &mut sources {
                if source.next.is_some() {
                    source.read_next()?;
                }
            }
            continue;
        };
        let source = &mut sources[earliest];
        if let Some(mut packet) = source.next.take()
            && let Some((place, from, to)) = source.streams[packet.stream()]
        {
            packet.set_stream(place);
            packet.rescale_ts(from, to);
            packet.set_position(-1);
            packet.write_interleaved(&mut output)?;
        }
        source.read_next()?;
    }
    output.write_trailer()?;
    Ok(())
}
