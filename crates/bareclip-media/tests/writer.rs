//! Writes whole videos with each encoder this machine has, because a machine without NVIDIA
//! hardware falls through to the others, and one that hangs would hang the export.

use std::{
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use bareclip_core::picture::BlackPicture;
use bareclip_media::{PictureReader, SoundLabel, VideoSettings, VideoWriter, probe};

const FRAMES: u32 = 90;

fn settings() -> VideoSettings {
    VideoSettings {
        width: 320,
        height: 180,
        frame_rate: (30, 1),
        bit_rate: 4_000_000,
        sound_tracks: vec![SoundLabel::default()],
        quality: bareclip_media::Quality::default(),
    }
}

fn write_with(encoder: &'static str, path: PathBuf) -> Result<bool, String> {
    let picture = BlackPicture::new(320, 180);
    let silence = [0.0_f32; 3200];
    let mut writer = match VideoWriter::create_with_any_of(&path, &settings(), &[encoder]) {
        Ok(writer) => writer,
        // This machine has no such hardware.
        Err(_) => return Ok(false),
    };
    for _ in 0..FRAMES {
        writer
            .write_picture(&picture.view())
            .map_err(|error| error.to_string())?;
        writer
            .write_sound(0, &silence)
            .map_err(|error| error.to_string())?;
    }
    writer.finish().map_err(|error| error.to_string())?;
    Ok(true)
}

#[test]
fn every_encoder_here_finishes_a_whole_video() {
    let folder = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bareclip-writer");
    std::fs::create_dir_all(&folder).expect("the folder can be created");
    for encoder in [
        "h264_nvenc",
        "h264_amf",
        "h264_qsv",
        "h264_mf",
        "libopenh264",
    ] {
        let path = folder.join(format!("{encoder}.mp4"));
        let (sender, result) = mpsc::channel();
        let written = path.clone();
        let started = Instant::now();
        thread::spawn(move || {
            let _ = sender.send(write_with(encoder, written));
        });
        match result.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(true)) => {
                let info = probe(&path).expect("the video can be read");
                assert_eq!(info.video.codec, "h264", "{encoder}");
                let mut reader = PictureReader::open(&path).expect("the video opens");
                let mut pictures = 0;
                while reader.next_picture().expect("the video decodes").is_some() {
                    pictures += 1;
                }
                assert_eq!(pictures, FRAMES, "{encoder}");
                println!("{encoder}: finished in {:?}", started.elapsed());
            }
            // The processor's encoder is built in, so it is the one that must always work.
            Ok(Ok(false)) if encoder == "libopenh264" => panic!("{encoder} wouldn't start"),
            Ok(Ok(false)) => println!("{encoder}: not available here"),
            Ok(Err(error)) => panic!("{encoder} failed: {error}"),
            Err(_) => panic!("{encoder} never finished"),
        }
    }
}

/// Windows' own encoder froze when one closed as another opened. Each thread keeps writing short
/// videos with it, so that openings and closings overlap.
#[test]
fn encoders_used_at_the_same_time_all_finish() {
    const THREADS: usize = 4;
    const ROUNDS: usize = 10;
    let folder = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bareclip-writer-together");
    std::fs::create_dir_all(&folder).expect("the folder can be created");
    let (sender, finished) = mpsc::channel();
    for thread_number in 0..THREADS {
        let sender = sender.clone();
        let folder = folder.clone();
        thread::spawn(move || {
            for round in 0..ROUNDS {
                let path = folder.join(format!("{thread_number}-{round}.mp4"));
                let mut writer = VideoWriter::create_with_any_of(
                    &path,
                    &settings(),
                    &["h264_mf", "libopenh264"],
                )
                .expect("an encoder opens");
                let picture = BlackPicture::new(320, 180);
                writer
                    .write_picture(&picture.view())
                    .expect("a picture can be written");
                writer.finish().expect("the video finishes");
            }
            let _ = sender.send(());
        });
    }
    for _ in 0..THREADS {
        finished
            .recv_timeout(Duration::from_secs(120))
            .expect("every thread finishes without freezing");
    }
}
