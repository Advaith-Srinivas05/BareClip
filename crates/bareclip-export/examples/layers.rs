//! The compositor's speed test: three stacked 1080p60 videos, decoded and drawn as fast as they
//! can go, the way a layered project will be once layers can be made. Needs a real GPU, and is
//! only meaningful in a release build.
//!
//! `cargo run --release -p bareclip-export --example layers -- [frames]`

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::Instant,
};

use bareclip_core::time::Ticks;
use bareclip_export::VideoSink;
use bareclip_media::{DecodedPicture, PictureReader, VideoSettings};
use bareclip_render::{Compositor, Layer, Rect, fit_inside};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const RATE: u32 = 60;
/// What each stage has to reach, from the plan's targets.
const PREVIEW_TARGET: f64 = 60.0;
const EXPORT_TARGET: f64 = 120.0;

/// A busy test video with B-frames, like a gameplay recording.
fn source_video(name: &str, pattern: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("bareclip-layers-{name}.mp4"));
    if path.exists() {
        return path;
    }
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg(format!("{pattern},format=yuv420p"))
        .args(["-t", "30"])
        .args([
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p4",
            "-rc",
            "vbr",
            "-cq",
            "20",
        ])
        .args(["-bf", "3", "-g", "120", "-colorspace", "bt709"])
        .arg(&path)
        .status()
        .expect("ffmpeg is on PATH");
    assert!(status.success(), "the test video could not be made");
    path
}

/// Decodes one video over and over on its own thread.
fn feed(path: PathBuf) -> mpsc::Receiver<DecodedPicture> {
    let (sender, receiver) = mpsc::sync_channel(6);
    thread::spawn(move || {
        let mut reader = PictureReader::open(&path).expect("the video opens");
        loop {
            match reader.next_picture().expect("it decodes") {
                Some(picture) => {
                    if sender.send(picture).is_err() {
                        return;
                    }
                }
                None => reader.seek(Ticks(0)).expect("it can start again"),
            }
        }
    });
    receiver
}

fn feeds(sources: &[PathBuf]) -> Vec<mpsc::Receiver<DecodedPicture>> {
    sources.iter().map(|path| feed(path.clone())).collect()
}

/// One full-screen video with two smaller ones over it.
fn layers_of(pictures: &[DecodedPicture]) -> Vec<Layer<'_>> {
    let (width, height) = (WIDTH as f32, HEIGHT as f32);
    let places = [
        fit_inside((WIDTH, HEIGHT), (WIDTH, HEIGHT)),
        Rect {
            x: width * 0.05,
            y: height * 0.05,
            width: width * 0.4,
            height: height * 0.4,
        },
        Rect {
            x: width * 0.55,
            y: height * 0.55,
            width: width * 0.4,
            height: height * 0.4,
        },
    ];
    pictures
        .iter()
        .zip(places)
        .map(|(picture, rect)| Layer {
            picture: picture.view().into(),
            rect,
            opacity: 1.0,
            look: bareclip_render::PictureLook::PLAIN,
        })
        .collect()
}

fn next_pictures(feeds: &[mpsc::Receiver<DecodedPicture>]) -> Vec<DecodedPicture> {
    feeds
        .iter()
        .map(|feed| feed.recv().expect("the decoder is running"))
        .collect()
}

fn report(name: &str, frames: u32, seconds: f64, target: f64) -> bool {
    let rate = f64::from(frames) / seconds;
    let passed = rate >= target;
    let verdict = if passed { "PASS" } else { "MISS" };
    println!("{name:<44} {rate:>7.1} fps   target {target:>5.0}   {verdict}");
    passed
}

fn decoding_only(sources: &[PathBuf], frames: u32) -> f64 {
    let feeds = feeds(sources);
    let started = Instant::now();
    for _ in 0..frames {
        next_pictures(&feeds);
    }
    started.elapsed().as_secs_f64()
}

fn preview_style(sources: &[PathBuf], frames: u32) -> (f64, String) {
    let mut compositor = Compositor::new(WIDTH, HEIGHT).expect("the GPU opens");
    let name = compositor.gpu_name().to_owned();
    let feeds = feeds(sources);
    let started = Instant::now();
    for _ in 0..frames {
        let pictures = next_pictures(&feeds);
        compositor.draw(&layers_of(&pictures)).expect("it draws");
    }
    compositor.wait().expect("the GPU finishes");
    (started.elapsed().as_secs_f64(), name)
}

fn export_style(sources: &[PathBuf], frames: u32, output: &Path) -> (f64, String) {
    let mut compositor = Compositor::new(WIDTH, HEIGHT).expect("the GPU opens");
    let mut writer = VideoSink::create(
        output,
        &VideoSettings {
            width: WIDTH,
            height: HEIGHT,
            frame_rate: (RATE as i32, 1),
            bit_rate: 20_000_000,
            sound_tracks: Vec::new(),
            quality: bareclip_media::Quality::default(),
            format: bareclip_media::VideoFormat::H264,
        },
    )
    .expect("the file can be written");
    let encoder = writer.encoder_name().to_owned();
    let feeds = feeds(sources);
    let started = Instant::now();
    for _ in 0..frames {
        let pictures = next_pictures(&feeds);
        let drawn = compositor.render(&layers_of(&pictures)).expect("it draws");
        writer.write_picture(&drawn).expect("it encodes");
    }
    writer.finish().expect("the file finishes");
    (started.elapsed().as_secs_f64(), encoder)
}

/// A mixed project's everyday frame: one smaller video scaled up, read back and encoded.
fn scaled_export(source: &Path, frames: u32, output: &Path) -> f64 {
    let mut compositor = Compositor::new(WIDTH, HEIGHT).expect("the GPU opens");
    let mut writer = VideoSink::create(
        output,
        &VideoSettings {
            width: WIDTH,
            height: HEIGHT,
            frame_rate: (RATE as i32, 1),
            bit_rate: 20_000_000,
            sound_tracks: Vec::new(),
            quality: bareclip_media::Quality::default(),
            format: bareclip_media::VideoFormat::H264,
        },
    )
    .expect("the file can be written");
    let feed = feed(source.to_owned());
    let started = Instant::now();
    for _ in 0..frames {
        let picture = feed.recv().expect("the decoder is running");
        let layer = Layer {
            picture: picture.view().into(),
            rect: fit_inside(picture.size(), (WIDTH, HEIGHT)),
            opacity: 1.0,
            look: bareclip_render::PictureLook::PLAIN,
        };
        let drawn = compositor.render(&[layer]).expect("it draws");
        writer.write_picture(&drawn).expect("it encodes");
    }
    writer.finish().expect("the file finishes");
    started.elapsed().as_secs_f64()
}

fn single_export(source: &Path, frames: u32, output: &Path) -> f64 {
    let mut writer = VideoSink::create(
        output,
        &VideoSettings {
            width: WIDTH,
            height: HEIGHT,
            frame_rate: (RATE as i32, 1),
            bit_rate: 20_000_000,
            sound_tracks: Vec::new(),
            quality: bareclip_media::Quality::default(),
            format: bareclip_media::VideoFormat::H264,
        },
    )
    .expect("the file can be written");
    let feed = feed(source.to_owned());
    let started = Instant::now();
    for _ in 0..frames {
        let picture = feed.recv().expect("the decoder is running");
        writer.write_picture(&picture.view()).expect("it encodes");
    }
    writer.finish().expect("the file finishes");
    started.elapsed().as_secs_f64()
}

fn main() {
    let frames: u32 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(1200);
    println!("Making test videos if needed...");
    let sources = [
        source_video("a", "testsrc2=s=1920x1080:r=60,noise=alls=10:allf=t"),
        source_video("b", "mandelbrot=s=1920x1080:r=60"),
        source_video(
            "c",
            "testsrc2=s=1920x1080:r=60,hue=H=2*PI*t,noise=alls=10:allf=t",
        ),
    ];
    let output = std::env::temp_dir().join("bareclip-layers-output.mp4");

    let single = single_export(&sources[0], frames, &output);
    report(
        "One 1080p60, no GPU drawing, encoded",
        frames,
        single,
        EXPORT_TARGET,
    );

    let small = source_video("small", "testsrc2=s=1280x720:r=60,noise=alls=10:allf=t");
    let scaled = scaled_export(&small, frames, &output);
    report(
        "One 720p60 scaled up to 1080p, encoded",
        frames,
        scaled,
        EXPORT_TARGET,
    );

    let decode = decoding_only(&sources, frames);
    println!(
        "{:<44} {:>7.1} fps   (the limit for the two below)",
        "Three 1080p60 decoded at once",
        f64::from(frames) / decode
    );

    let (preview, gpu) = preview_style(&sources, frames);
    let preview_ok = report(
        "Three layers drawn (what a preview needs)",
        frames,
        preview,
        PREVIEW_TARGET,
    );

    let (export, encoder) = export_style(&sources, frames, &output);
    let export_ok = report(
        "Three layers drawn, read back and encoded",
        frames,
        export,
        EXPORT_TARGET,
    );

    println!("GPU: {gpu}");
    println!("Encoder: {encoder}");
    let _ = std::fs::remove_file(&output);
    if !(preview_ok && export_ok) {
        std::process::exit(1);
    }
}
