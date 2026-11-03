//! Test clips whose frames identify themselves, and tools to read them back.
//!
//! Every frame shows its number as a row of black and white blocks, which survives
//! lossy compression. Reading the blocks after an export proves exactly which
//! source frames came out, in which order.

#![allow(dead_code)]

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, OnceLock},
};

use bareclip_core::{
    splice::{CutMode, FrameIndex},
    time::{TICKS_PER_SECOND, Ticks},
    timeline::TimeRange,
};
use bareclip_media::{ExportPart, ExportReport, ExportRequest, export, scan_frames};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{decoder, format, frame, media::Type};

pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 180;
const BLOCK_WIDTH: usize = 20;
const BITS: usize = WIDTH / BLOCK_WIDTH;

pub const FPS: i64 = 30;

/// Writes a frame number into the picture as blocks.
const BARCODE: &str = "geq=lum='255*mod(floor(N/pow(2,floor(X/20))),2)':cb=128:cr=128";

pub fn output_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bareclip-media");
    std::fs::create_dir_all(&dir).expect("the test folder can be created");
    dir
}

/// A unique path for a test's output.
pub fn output_path(name: &str, extension: &str) -> PathBuf {
    let path = output_dir().join(format!("out-{name}.{extension}"));
    let _ = std::fs::remove_file(&path);
    path
}

fn run_ffmpeg(args: &[&str], output: &Path) {
    let result = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(output)
        .output()
        .expect("ffmpeg is on PATH");
    assert!(
        result.status.success(),
        "ffmpeg failed for {}: {}",
        output.display(),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn video_source(seconds: u32) -> String {
    video_source_sized(seconds, HEIGHT)
}

fn video_source_sized(seconds: u32, height: usize) -> String {
    format!("nullsrc=s={WIDTH}x{height}:r={FPS}:d={seconds},{BARCODE},format=yuv420p")
}

/// Beeps for a tenth of a second at the start of every second.
fn beep_source(seconds: u32) -> String {
    format!("aevalsrc='0.8*sin(2*PI*440*t)*lt(mod(t,1),0.1)':d={seconds}:s=48000")
}

fn tone_source(frequency: u32, seconds: u32) -> String {
    format!("sine=frequency={frequency}:sample_rate=48000:duration={seconds}")
}

/// Creates a clip once, however many tests ask for it.
fn cached(name: &str, create: impl FnOnce(&Path)) -> PathBuf {
    static LOCKS: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut made = locks.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(path) = made.get(name) {
        return path.clone();
    }
    let path = output_dir().join(name);
    create(&path);
    made.insert(name.to_owned(), path.clone());
    path
}

/// H.264 without B-frames, a keyframe every second, with one beeping audio track.
pub fn h264_mp4() -> PathBuf {
    cached("h264.mp4", |path| {
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                &video_source(10),
                "-f",
                "lavfi",
                "-i",
                &beep_source(10),
                "-c:v",
                "libopenh264",
                "-b:v",
                "4M",
                "-g",
                "30",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
            ],
            path,
        );
    })
}

/// H.264 with no audio at all.
pub fn silent_mp4() -> PathBuf {
    cached("silent.mp4", |path| {
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                &video_source(3),
                "-c:v",
                "libopenh264",
                "-b:v",
                "1M",
                "-g",
                "30",
            ],
            path,
        );
    })
}

/// HEVC with B-frames in Matroska and two named audio tracks at different pitches,
/// like an OBS recording with game and microphone audio kept apart.
pub fn b_frames_two_tracks_mkv() -> PathBuf {
    cached("bframes.mkv", |path| {
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                &video_source_sized(10, 184),
                "-f",
                "lavfi",
                "-i",
                &tone_source(440, 10),
                "-f",
                "lavfi",
                "-i",
                &tone_source(1760, 10),
                "-map",
                "0",
                "-map",
                "1",
                "-map",
                "2",
                "-c:v",
                "libkvazaar",
                "-kvazaar-params",
                "qp=18,period=32,gop=8,open-gop=0",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-metadata:s:a:0",
                "title=Game",
                "-metadata:s:a:0",
                "language=eng",
                "-metadata:s:a:1",
                "title=Microphone",
            ],
            path,
        );
    })
}

/// MPEG-4 Part 2 with B-frames, a codec that frame-exact cuts do not support.
pub fn mpeg4_mkv() -> PathBuf {
    cached("mpeg4.mkv", |path| {
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                &video_source(10),
                "-c:v",
                "mpeg4",
                "-q:v",
                "2",
                "-bf",
                "2",
                "-g",
                "30",
                "-flags",
                "+cgop",
                "-sc_threshold",
                "1000000000",
            ],
            path,
        );
    })
}

/// AV1 from a software encoder, as OBS can record on newer GPUs.
pub fn av1_mkv() -> PathBuf {
    cached("av1.mkv", |path| {
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                &video_source(6),
                "-c:v",
                "libsvtav1",
                "-preset",
                "10",
                "-crf",
                "20",
                "-g",
                "30",
            ],
            path,
        );
    })
}

/// Uneven frame timing, like a screen recording.
pub fn variable_rate_mkv() -> PathBuf {
    cached("vfr.mkv", |path| {
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                &format!("{},setpts='(N+0.4*mod(N,3))/{FPS}/TB'", video_source(10)),
                "-fps_mode",
                "passthrough",
                "-c:v",
                "libopenh264",
                "-b:v",
                "4M",
                "-g",
                "30",
            ],
            path,
        );
    })
}

/// HEVC from a software encoder, available wherever the tests run.
pub fn hevc_mkv() -> PathBuf {
    cached("hevc.mkv", |path| {
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                &video_source_sized(6, 184),
                "-c:v",
                "libkvazaar",
                "-g",
                "30",
                "-kvazaar-params",
                "qp=18,period=30,gop=0",
            ],
            path,
        );
    })
}

/// Another clip in the same format as `h264_mp4`, with frame numbers from 1000, so
/// a test can tell which source each frame in a joined export came from.
pub fn second_h264_mp4() -> PathBuf {
    cached("h264-second.mp4", |path| {
        let video = format!(
            "nullsrc=s={WIDTH}x{HEIGHT}:r={FPS}:d=6,\
             geq=lum='255*mod(floor((N+1000)/pow(2,floor(X/20))),2)':cb=128:cr=128,format=yuv420p"
        );
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                &video,
                "-f",
                "lavfi",
                "-i",
                &beep_source(6),
                "-c:v",
                "libopenh264",
                "-b:v",
                "4M",
                "-g",
                "30",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
            ],
            path,
        );
    })
}

/// The same pictures at a different size, which cannot be joined by copying.
pub fn different_size_mp4() -> PathBuf {
    cached("h264-small.mp4", |path| {
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=160x90:r=30:d=2,format=yuv420p",
                "-f",
                "lavfi",
                "-i",
                &beep_source(2),
                "-c:v",
                "libopenh264",
                "-b:v",
                "1M",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
            ],
            path,
        );
    })
}

/// A flat colour encoded and labelled with the HD colour standard.
pub fn bt709_color_clip() -> PathBuf {
    cached("bt709.mp4", |path| {
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "color=c=0x3080C0:s=320x180:r=30:d=1,scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
                "-c:v",
                "libopenh264",
                "-b:v",
                "2M",
                "-colorspace",
                "bt709",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
            ],
            path,
        );
    })
}

/// A hardware-encoded clip with B-frames, which is how OBS commonly records.
pub fn nvenc_clip(encoder: &str, extension: &str) -> PathBuf {
    cached(&format!("nvenc-{encoder}.{extension}"), |path| {
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                &video_source(10),
                "-f",
                "lavfi",
                "-i",
                &beep_source(10),
                "-c:v",
                encoder,
                "-preset",
                "p4",
                "-rc",
                "constqp",
                "-qp",
                "20",
                "-bf",
                "3",
                "-g",
                "60",
                "-c:a",
                "aac",
            ],
            path,
        );
    })
}

/// Frame numbers that frame-exact cuts must copy for a section, given a video with
/// a steady frame rate: those between the first and last clean points inside it.
pub fn frames_copied_within(
    index: &FrameIndex,
    time_base: f64,
    start: u32,
    end: u32,
) -> std::ops::Range<u32> {
    let to_frame = |pts: i64| (pts as f64 * time_base * FPS as f64).round() as u32;
    let clean: Vec<u32> = index
        .clean_points()
        .into_iter()
        .map(to_frame)
        .filter(|frame| (start..=end).contains(frame))
        .collect();
    let first = clean.first().copied().unwrap_or(start);
    let last = clean.last().copied().unwrap_or(start);
    first..last
}

pub fn index_of(path: &Path) -> FrameIndex {
    scan_frames(path, &std::sync::atomic::AtomicBool::new(false), |_| {})
        .expect("the frames can be listed")
}

pub fn seconds(value: f64) -> Ticks {
    Ticks((value * TICKS_PER_SECOND as f64).round() as i64)
}

pub fn range(start: f64, end: f64) -> TimeRange {
    TimeRange {
        start: seconds(start),
        end: seconds(end),
    }
}

pub fn parts_of(input: &Path, ranges: Vec<TimeRange>) -> Vec<ExportPart> {
    let frames = Arc::new(index_of(input));
    ranges
        .into_iter()
        .map(|range| ExportPart {
            input: input.to_owned(),
            frames: frames.clone(),
            range,
            gap_before: Ticks(0),
        })
        .collect()
}

pub fn run_export(
    input: &Path,
    output: &Path,
    ranges: Vec<TimeRange>,
    mode: CutMode,
) -> ExportReport {
    run_parts(output, parts_of(input, ranges), mode)
}

pub fn run_parts(output: &Path, parts: Vec<ExportPart>, mode: CutMode) -> ExportReport {
    export(
        &ExportRequest {
            output: output.to_owned(),
            parts,
            mode,
        },
        &std::sync::atomic::AtomicBool::new(false),
        &|_| {},
    )
    .expect("the export succeeds")
}

#[derive(Clone, Copy, Debug)]
pub struct DecodedFrame {
    pub seconds: f64,
    pub number: u32,
    /// Changes if any pixel in the picture changes.
    pub hash: u64,
}

fn read_frame(video: &frame::Video, seconds: f64) -> DecodedFrame {
    let stride = video.stride(0);
    let data = video.data(0);
    let width = video.width() as usize;
    let height = video.height() as usize;

    let middle = stride * (height / 2);
    let mut number = 0;
    for bit in 0..BITS {
        let x = bit * BLOCK_WIDTH + BLOCK_WIDTH / 2;
        if data[middle + x] > 128 {
            number |= 1 << bit;
        }
    }

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for row in 0..height {
        for byte in &data[stride * row..stride * row + width] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }

    DecodedFrame {
        seconds,
        number,
        hash,
    }
}

/// Decodes every frame of a video, in display order.
pub fn decode_video(path: &Path) -> Vec<DecodedFrame> {
    let mut input = format::input(path).expect("the file opens");
    let stream = input.streams().best(Type::Video).expect("it has video");
    let index = stream.index();
    let time_base = f64::from(stream.time_base());
    let mut decoder: decoder::Video =
        ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .expect("the video has valid settings")
            .decoder()
            .video()
            .expect("a decoder exists");

    let mut frames = Vec::new();
    let mut decoded = frame::Video::empty();
    for (stream, packet) in input.packets() {
        if stream.index() != index {
            continue;
        }
        decoder.send_packet(&packet).expect("the packet decodes");
        while decoder.receive_frame(&mut decoded).is_ok() {
            let seconds = decoded.timestamp().unwrap_or_default() as f64 * time_base;
            frames.push(read_frame(&decoded, seconds));
        }
    }
    decoder.send_eof().expect("the decoder can finish");
    while decoder.receive_frame(&mut decoded).is_ok() {
        let seconds = decoded.timestamp().unwrap_or_default() as f64 * time_base;
        frames.push(read_frame(&decoded, seconds));
    }
    frames.sort_by(|a, b| a.seconds.total_cmp(&b.seconds));
    frames
}

pub fn numbers(frames: &[DecodedFrame]) -> Vec<u32> {
    frames.iter().map(|frame| frame.number).collect()
}

/// Asserts that a frame number's picture is byte-for-byte the same in both lists.
pub fn assert_identical(source: &[DecodedFrame], output: &[DecodedFrame], number: u32) {
    let find = |frames: &[DecodedFrame]| {
        frames
            .iter()
            .find(|frame| frame.number == number)
            .unwrap_or_else(|| panic!("frame {number} is missing"))
            .hash
    };
    assert_eq!(find(source), find(output), "frame {number} changed");
}

#[derive(Clone, Debug)]
pub struct AudioTrackInfo {
    pub title: Option<String>,
    pub language: Option<String>,
    pub seconds: f64,
    /// The start time of each decoded sample block and its first channel's samples.
    pub samples: Vec<f32>,
    pub first_sample_seconds: f64,
    pub rate: u32,
}

/// Decodes the first channel of every audio track.
pub fn decode_audio(path: &Path) -> Vec<AudioTrackInfo> {
    let mut input = format::input(path).expect("the file opens");
    let mut decoders: HashMap<usize, (decoder::Audio, AudioTrackInfo, f64)> = HashMap::new();
    let mut order = Vec::new();
    for stream in input.streams() {
        if stream.parameters().medium() != Type::Audio {
            continue;
        }
        let audio = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .expect("the audio has valid settings")
            .decoder()
            .audio()
            .expect("a decoder exists");
        let metadata = stream.metadata();
        order.push(stream.index());
        decoders.insert(
            stream.index(),
            (
                audio,
                AudioTrackInfo {
                    title: metadata.get("title").map(str::to_owned),
                    language: metadata.get("language").map(str::to_owned),
                    seconds: 0.0,
                    samples: Vec::new(),
                    first_sample_seconds: f64::MAX,
                    rate: 0,
                },
                f64::from(stream.time_base()),
            ),
        );
    }

    fn drain(audio: &mut decoder::Audio, info: &mut AudioTrackInfo, time_base: f64) {
        let mut decoded = frame::Audio::empty();
        while audio.receive_frame(&mut decoded).is_ok() {
            info.rate = decoded.rate();
            let start = decoded.timestamp().unwrap_or_default() as f64 * time_base;
            info.first_sample_seconds = info.first_sample_seconds.min(start);
            let plane: &[f32] = decoded.plane(0);
            // Sample positions follow timestamps, so gaps at joins are preserved.
            let position = ((start - info.first_sample_seconds) * f64::from(info.rate)) as usize;
            if info.samples.len() < position {
                info.samples.resize(position, 0.0);
            }
            info.samples.extend_from_slice(&plane[..decoded.samples()]);
            info.seconds = info.samples.len() as f64 / f64::from(info.rate);
        }
    }

    for (stream, packet) in input.packets() {
        if let Some((audio, info, time_base)) = decoders.get_mut(&stream.index()) {
            audio.send_packet(&packet).expect("the audio decodes");
            drain(audio, info, *time_base);
        }
    }
    for (audio, info, time_base) in decoders.values_mut() {
        audio.send_eof().expect("the decoder can finish");
        drain(audio, info, *time_base);
    }

    order
        .iter()
        .map(|index| decoders.remove(index).expect("the track exists").1)
        .collect()
}

/// The pitch of a section of audio, measured by counting zero crossings.
pub fn pitch_of(samples: &[f32], rate: u32) -> f64 {
    let crossings = samples
        .windows(2)
        .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
        .count();
    crossings as f64 / 2.0 / (samples.len() as f64 / f64::from(rate))
}

/// The time of each beep: where the sound starts after a stretch of silence.
pub fn beep_times(track: &AudioTrackInfo) -> Vec<f64> {
    let rate = f64::from(track.rate);
    let quiet_needed = (rate * 0.2) as usize;
    let mut quiet = quiet_needed;
    let mut times = Vec::new();
    for (position, sample) in track.samples.iter().enumerate() {
        if sample.abs() > 0.2 {
            if quiet >= quiet_needed {
                times.push(track.first_sample_seconds + position as f64 / rate);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    times
}
