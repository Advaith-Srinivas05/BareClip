use super::*;
use std::{path::PathBuf, process::Command};

fn folder() -> PathBuf {
    let folder = std::env::temp_dir()
        .join("bareclip-details-tests")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

fn ffmpeg(arguments: &[&str], output: &Path) {
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(arguments)
        .arg(output)
        .status()
        .expect("ffmpeg is on PATH");
    assert!(status.success(), "ffmpeg made {}", output.display());
}

/// A 3 second 320x180 30 fps video with sound and two chapters.
fn video_with_chapters(name: &str) -> PathBuf {
    let folder = folder();
    let chapters = folder.join(format!("{name}-chapters.txt"));
    std::fs::write(
        &chapters,
        ";FFMETADATA1\ntitle=Holiday\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1500\ntitle=Arriving\n\
         [CHAPTER]\nTIMEBASE=1/1000\nSTART=1500\nEND=3000\ntitle=Leaving\n",
    )
    .unwrap();
    let path = folder.join(format!("{name}.mp4"));
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=320x180:r=30:d=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=3",
            "-i",
            chapters.to_str().unwrap(),
            "-map_metadata",
            "2",
            "-map_chapters",
            "2",
            "-c:v",
            "libopenh264",
            "-c:a",
            "aac",
            "-ac",
            "2",
        ],
        &path,
    );
    path
}

/// How many packets each stream has, and how long they last, as FFmpeg counts them.
fn packet_counts(path: &Path) -> String {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-count_packets", "-show_entries"])
        .args([
            "stream=index,codec_name,nb_read_packets,duration_ts",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .unwrap();
    // An MP4's chapter track is written afresh from the chapters, which are checked apart.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.contains("bin_data"))
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

#[test]
fn a_videos_details_describe_its_picture_sound_and_chapters() {
    let path = video_with_chapters("details");
    let details = read_details(&path).unwrap();

    assert!(details.container.contains("MP4") || details.container.contains("QuickTime"));
    assert!((details.duration.as_seconds() - 3.0).abs() < 0.1);
    assert_eq!(details.size_bytes, std::fs::metadata(&path).unwrap().len());
    assert!(details.bit_rate.is_some());
    assert_eq!(details.chapters, 2);
    assert_eq!(details.subtitles, 0);
    assert_eq!(details.tag("title"), Some("Holiday"));
    match &details.streams[0] {
        StreamDetails::Picture {
            codec,
            width,
            height,
            frame_rate,
            variable_rate,
            bit_depth,
            ..
        } => {
            assert_eq!(codec, "h264");
            assert_eq!((*width, *height), (320, 180));
            assert!((frame_rate - 30.0).abs() < 0.01);
            assert!(!variable_rate);
            assert_eq!(*bit_depth, Some(8));
        }
        other => panic!("the picture comes first: {other:?}"),
    }
    match &details.streams[1] {
        StreamDetails::Sound {
            codec,
            channels,
            layout,
            sample_rate,
            ..
        } => {
            assert_eq!(codec, "aac");
            assert_eq!(*channels, 2);
            assert_eq!(layout.as_deref(), Some("stereo"));
            assert_eq!(*sample_rate, 48000);
        }
        other => panic!("then the sound: {other:?}"),
    }
}

#[test]
fn a_songs_details_list_its_tags_and_leave_out_its_cover() {
    let path = folder().join("song.mp3");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=duration=2",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=64x64:r=1:d=1",
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-c:v",
            "png",
            "-disposition:v",
            "attached_pic",
            "-c:a",
            "libmp3lame",
            "-metadata",
            "artist=The Band",
            "-metadata",
            "title=A Song",
            "-metadata",
            "album=First",
        ],
        &path,
    );
    let details = read_details(&path).unwrap();
    assert_eq!(details.tag("artist"), Some("The Band"));
    assert_eq!(details.tag("title"), Some("A Song"));
    assert_eq!(details.tag("album"), Some("First"));
    assert_eq!(details.streams.len(), 1, "the cover isn't a picture stream");
    assert!(matches!(&details.streams[0], StreamDetails::Sound { codec, .. } if codec == "mp3"));
}

#[test]
fn a_recording_with_two_named_sound_tracks_lists_both() {
    let path = folder().join("recording.mkv");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=160x90:r=30:d=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=300:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=600:duration=1",
            "-map",
            "0",
            "-map",
            "1",
            "-map",
            "2",
            "-c:v",
            "libopenh264",
            "-c:a",
            "aac",
            "-metadata:s:a:0",
            "title=Game",
            "-metadata:s:a:1",
            "title=Microphone",
            "-metadata:s:a:1",
            "language=eng",
        ],
        &path,
    );
    let details = read_details(&path).unwrap();
    let names: Vec<(Option<String>, Option<String>)> = details
        .streams
        .iter()
        .filter_map(|stream| match stream {
            StreamDetails::Sound { name, language, .. } => Some((name.clone(), language.clone())),
            StreamDetails::Picture { .. } => None,
        })
        .collect();
    assert_eq!(
        names,
        [
            (Some("Game".to_owned()), None),
            (Some("Microphone".to_owned()), Some("eng".to_owned()))
        ]
    );
}

#[test]
fn rewritten_tags_read_back_and_every_stream_and_chapter_survives() {
    let source = video_with_chapters("rewrite");
    let copy = folder().join("rewrite-copy.mp4");
    let changes = TagChanges {
        artist: Some("Somebody".to_owned()),
        creation_time: Some("2025-09-14T01:35:00.000000Z".to_owned()),
    };
    let mut reported = Vec::new();
    write_with_tags(&source, &copy, &changes, &AtomicBool::new(false), |share| {
        reported.push(share);
    })
    .unwrap();

    let details = read_details(&copy).unwrap();
    assert_eq!(details.tag("artist"), Some("Somebody"));
    assert!(
        details
            .tag("creation_time")
            .is_some_and(|time| time.starts_with("2025-09-14T01:35:00")),
        "{:?}",
        details.tag("creation_time")
    );
    assert_eq!(details.tag("title"), Some("Holiday"), "other tags stay");
    assert_eq!(details.chapters, 2);
    assert_eq!(
        packet_counts(&copy),
        packet_counts(&source),
        "every packet copied"
    );
    assert_eq!(reported.last(), Some(&1.0));

    // Taking a tag away.
    let again = folder().join("rewrite-again.mp4");
    let removal = TagChanges {
        artist: Some(String::new()),
        creation_time: None,
    };
    write_with_tags(&copy, &again, &removal, &AtomicBool::new(false), |_| {}).unwrap();
    let details = read_details(&again).unwrap();
    assert_eq!(details.tag("artist"), None);
    assert!(details.tag("creation_time").is_some());
}

#[test]
fn matroska_keeps_its_creation_time() {
    let source = folder().join("tags.mkv");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=160x90:r=30:d=1",
            "-c:v",
            "libopenh264",
        ],
        &source,
    );
    let copy = folder().join("tags-copy.mkv");
    let changes = TagChanges {
        artist: Some("Somebody".to_owned()),
        creation_time: Some("2024-01-02T03:04:05.000000Z".to_owned()),
    };
    write_with_tags(&source, &copy, &changes, &AtomicBool::new(false), |_| {}).unwrap();
    let details = read_details(&copy).unwrap();
    assert_eq!(
        details.tag("artist").or(details.tag("ARTIST")),
        Some("Somebody")
    );
    assert!(
        details
            .tag("creation_time")
            .is_some_and(|time| time.starts_with("2024-01-02T03:04:05")),
        "{:?}",
        details.tags
    );
}

#[test]
fn a_cancelled_rewrite_leaves_nothing_and_the_original_untouched() {
    let source = video_with_chapters("cancelled");
    let before = std::fs::read(&source).unwrap();
    let copy = folder().join("cancelled-copy.mp4");
    let changes = TagChanges {
        artist: Some("Nobody".to_owned()),
        creation_time: None,
    };
    let result = write_with_tags(&source, &copy, &changes, &AtomicBool::new(true), |_| {});
    assert!(matches!(result, Err(MediaError::Cancelled)));
    assert!(!copy.exists(), "no copy is left");
    assert_eq!(std::fs::read(&source).unwrap(), before);
}

#[test]
fn only_kinds_of_file_whose_tags_ffmpeg_writes_can_be_changed() {
    let support = |name: &str| tag_support(Path::new(name));
    assert!(support("a.MP4").creation_time);
    assert!(support("a.mkv").artist);
    assert!(support("a.mp3").artist);
    assert!(!support("a.mp3").creation_time);
    for name in ["a.avi", "a.wmv", "a.flv", "a.wav", "a.ts", "a.mts"] {
        assert_eq!(
            support(name),
            TagSupport {
                artist: false,
                creation_time: false
            },
            "{name}"
        );
    }
}
