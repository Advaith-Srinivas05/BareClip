use super::*;
use std::{path::PathBuf, process::Command};

fn folder() -> PathBuf {
    let folder = std::env::temp_dir()
        .join("bareclip-sound-cut-tests")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

/// Three seconds of a sweeping tone at 48 kHz in stereo, so every moment differs.
fn sound(name: &str, codec: &[&str]) -> PathBuf {
    let path = folder().join(name);
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
        .arg("aevalsrc='sin(2*PI*(200+300*t)*t)|cos(2*PI*(150+200*t)*t)':s=48000:d=3")
        .args(codec)
        .arg(&path)
        .status()
        .expect("ffmpeg is on PATH");
    assert!(status.success());
    path
}

/// Every sample of a file, decoded by FFmpeg as 16-bit stereo.
fn samples(path: &Path) -> Vec<i16> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-f", "s16le", "-ac", "2", "-ar", "48000", "-"])
        .output()
        .unwrap();
    assert!(output.status.success());
    output
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect()
}

fn length(path: &Path) -> f64 {
    crate::probe(path).unwrap().duration.as_seconds()
}

fn range(start: f64, end: f64) -> TimeRange {
    TimeRange {
        start: Ticks::from_seconds(start),
        end: Ticks::from_seconds(end),
    }
}

fn cut_to(input: &Path, name: &str, start: f64, end: f64) -> (PathBuf, SoundCut) {
    let output = folder().join(name);
    let how = cut_sound(
        input,
        &output,
        range(start, end),
        &AtomicBool::new(false),
        &|_| {},
    )
    .unwrap();
    (output, how)
}

#[test]
fn wav_and_aiff_are_cut_to_the_exact_sample() {
    for (name, codec) in [
        ("tone.wav", &["-c:a", "pcm_s16le"][..]),
        ("tone.aiff", &["-c:a", "pcm_s16be"][..]),
    ] {
        let original = sound(name, codec);
        let (cut, how) = cut_to(&original, &format!("cut-{name}"), 0.5, 1.25);
        assert_eq!(how, SoundCut::Exact, "{name}");
        let whole = samples(&original);
        let kept = samples(&cut);
        assert_eq!(kept.len(), 36_000 * 2, "{name}: exactly 0.75 s");
        assert_eq!(
            kept,
            whole[24_000 * 2..60_000 * 2],
            "{name}: every sample unchanged"
        );
    }
}

#[test]
fn flac_is_cut_to_the_exact_sample() {
    let original = sound("tone.flac", &["-c:a", "flac"]);
    let (cut, how) = cut_to(&original, "cut.flac", 0.5, 1.25);
    assert_eq!(how, SoundCut::Exact);
    let whole = samples(&original);
    let kept = samples(&cut);
    assert_eq!(kept.len(), 36_000 * 2);
    assert_eq!(kept, whole[24_000 * 2..60_000 * 2]);
}

#[test]
fn mp3_aac_and_opus_are_copied_to_within_a_packet_of_each_end() {
    for (name, codec, packet) in [
        (
            "tone.mp3",
            &["-c:a", "libmp3lame", "-ar", "48000"][..],
            1152.0 / 48000.0,
        ),
        ("tone.m4a", &["-c:a", "aac"][..], 1024.0 / 48000.0),
        ("tone.aac", &["-c:a", "aac"][..], 1024.0 / 48000.0),
        ("tone.opus", &["-c:a", "libopus"][..], 0.02),
        ("tone.ogg", &["-c:a", "libvorbis"][..], 0.03),
        ("tone.wma", &["-c:a", "wmav2"][..], 2048.0 / 48000.0),
        ("tone.mka", &["-c:a", "libopus"][..], 0.02),
    ] {
        let original = sound(name, codec);
        let (cut, how) = cut_to(&original, &format!("cut-{name}"), 0.8, 2.0);
        assert_eq!(how, SoundCut::Packets, "{name}");
        let kept = length(&cut);
        // It covers both ends, with at most a packet more at each, plus what the format adds
        // to the start for its decoder to settle.
        assert!(
            kept >= 1.2 - 0.001 && kept <= 1.2 + 2.0 * packet + 0.06,
            "{name}: {kept} s"
        );
        let heard = samples(&cut);
        let loud = heard
            .iter()
            .map(|sample| i32::from(*sample).abs())
            .max()
            .unwrap();
        assert!(loud > 10_000, "{name}: the sound is there");
    }
}

#[test]
fn a_cancelled_or_impossible_cut_leaves_no_file() {
    let original = sound("cancel.wav", &["-c:a", "pcm_s16le"]);
    let output = folder().join("cancelled.wav");
    let result = cut_sound(
        &original,
        &output,
        range(0.5, 1.0),
        &AtomicBool::new(true),
        &|_| {},
    );
    assert!(matches!(result, Err(MediaError::Cancelled)));
    assert!(!output.exists());

    let result = cut_sound(
        &original,
        &original,
        range(0.5, 1.0),
        &AtomicBool::new(false),
        &|_| {},
    );
    assert!(matches!(result, Err(MediaError::SameInputAndOutput)));
    assert!(original.exists(), "the original is untouched");
}
