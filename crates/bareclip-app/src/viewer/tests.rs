use super::*;
use crate::components::DateTime;
use crate::screen_camera::{self, Camera, KEPT_TEXTURES};
use std::{process::Command, sync::OnceLock};

/// A 4 second 320x180 video at 30 fps with a tone, whose frames count up in binary, so a
/// frame can be told from its neighbours.
fn make_video(name: &str, extension: &str) -> PathBuf {
    let folder = std::env::temp_dir().join("bareclip-viewer-tests");
    std::fs::create_dir_all(&folder).expect("the folder can be created");
    let path = folder.join(format!("{name}-{}.{extension}", std::process::id()));
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i"])
        .arg(
            "nullsrc=s=320x180:r=30:d=4,\
             geq=lum='255*mod(floor(N/pow(2,floor(X/20))),2)':cb=128:cr=128,format=yuv420p",
        )
        .args(["-f", "lavfi", "-i", "sine=frequency=330:duration=4"])
        .args(["-c:v", "libopenh264", "-b:v", "2M", "-g", "30"])
        .args(["-c:a", "aac", "-shortest"])
        .arg(&path)
        .status()
        .expect("ffmpeg is on PATH");
    assert!(status.success());
    path
}

fn video() -> &'static Path {
    static VIDEO: OnceLock<PathBuf> = OnceLock::new();
    VIDEO.get_or_init(|| make_video("viewer", "mp4"))
}

/// A 3 second MP3, with a cover if asked.
fn make_song(name: &str, with_cover: bool) -> PathBuf {
    let folder = std::env::temp_dir().join("bareclip-viewer-tests");
    std::fs::create_dir_all(&folder).expect("the folder can be created");
    let path = folder.join(format!("{name}-{}.mp3", std::process::id()));
    let mut command = Command::new("ffmpeg");
    command.args(["-hide_banner", "-loglevel", "error", "-y"]);
    command.args(["-f", "lavfi", "-i", "sine=frequency=440:duration=3"]);
    if with_cover {
        command.args(["-f", "lavfi", "-i", "testsrc2=s=300x300:r=1:d=1"]);
        command.args(["-map", "0:a", "-map", "1:v"]);
        command.args(["-c:v", "png", "-disposition:v", "attached_pic"]);
        command.args(["-id3v2_version", "3"]);
    }
    command.args(["-c:a", "libmp3lame"]).arg(&path);
    assert!(command.status().expect("ffmpeg is on PATH").success());
    path
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    crate::fonts::install(&ctx);
    crate::theme::install(&ctx);
    ctx
}

fn frame(ctx: &egui::Context, viewer: &mut Viewer, events: Vec<egui::Event>) -> egui::FullOutput {
    screen_camera::run(ctx, events, |ui| viewer.show(ui))
}

/// A player that has opened a file and shows its first picture, paused at the start.
fn opened(ctx: &egui::Context, path: &Path) -> Viewer {
    let mut viewer = Viewer::new(ctx, false);
    viewer.open(ctx, path);
    viewer.pause();
    viewer.seek(Ticks(0));
    wait_for(ctx, &mut viewer, "the first picture", |viewer| {
        viewer.picture.is_some() || viewer.file.as_ref().is_some_and(OpenFile::is_sound)
    });
    viewer
}

/// Runs frames until something is true, failing after 10 s.
fn wait_for(ctx: &egui::Context, viewer: &mut Viewer, what: &str, done: impl Fn(&Viewer) -> bool) {
    let started = Instant::now();
    while !done(viewer) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "waited too long for {what}"
        );
        frame(ctx, viewer, vec![]);
        thread::sleep(Duration::from_millis(5));
    }
}

fn press(key: Key) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]
}

fn named(ctx: &egui::Context, viewer: &mut Viewer) -> Vec<(String, Rect, bool)> {
    ctx.enable_accesskit();
    let output = frame(ctx, viewer, vec![]);
    screen_camera::named_nodes(&output)
}

fn named_rect(ctx: &egui::Context, viewer: &mut Viewer, name: &str) -> Rect {
    named(ctx, viewer)
        .into_iter()
        .find(|(label, ..)| label == name)
        .unwrap_or_else(|| panic!("nothing is named {name}"))
        .1
}

fn click_at(ctx: &egui::Context, viewer: &mut Viewer, at: egui::Pos2) {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    frame(ctx, viewer, vec![egui::Event::PointerMoved(at)]);
    frame(ctx, viewer, vec![button(true)]);
    frame(ctx, viewer, vec![button(false)]);
    frame(ctx, viewer, vec![]);
}

fn click_named(ctx: &egui::Context, viewer: &mut Viewer, name: &str) {
    let rect = named_rect(ctx, viewer, name);
    click_at(ctx, viewer, rect.center());
}

fn seconds(value: f64) -> Ticks {
    Ticks::from_seconds(value)
}

fn near(a: Ticks, b: Ticks, within: f64) -> bool {
    (a.as_seconds() - b.as_seconds()).abs() <= within
}

fn temporary_folder(name: &str) -> PathBuf {
    let folder = std::env::temp_dir()
        .join("bareclip-viewer-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("the folder can be created");
    folder
}

#[test]
fn a_video_opens_playing_and_shows_its_name_and_details() {
    let ctx = context();
    let mut viewer = Viewer::new(&ctx, false);
    viewer.open(&ctx, video());

    assert!(viewer.is_playing(), "a file opens playing");
    assert_eq!(
        viewer.name(),
        video().file_name().unwrap().to_string_lossy()
    );
    let details = file_details(viewer.file.as_ref().unwrap());
    assert_eq!(details[0], "MP4");
    assert_eq!(details[1], "320 x 180");
    wait_for(&ctx, &mut viewer, "a picture", |viewer| {
        viewer.picture.is_some()
    });
}

#[test]
fn a_file_that_cant_be_read_says_why() {
    let ctx = context();
    let folder = temporary_folder("broken");
    let broken = folder.join("broken.mp4");
    std::fs::write(&broken, b"this is not a video").unwrap();
    let mut viewer = Viewer::new(&ctx, false);

    viewer.open(&ctx, &broken);
    let failed = viewer.failed.as_ref().expect("it says it can't be played");
    assert!(failed.reason.contains("damaged"), "{}", failed.reason);
    assert_eq!(viewer.name(), "broken.mp4");

    viewer.open(&ctx, &folder.join("gone.mp4"));
    let failed = viewer.failed.as_ref().unwrap();
    assert!(failed.reason.contains("isn't there"), "{}", failed.reason);

    // Opening a good file afterwards clears the message.
    viewer.open(&ctx, video());
    assert!(viewer.failed.is_none());
    assert!(viewer.file.is_some());
}

#[test]
fn space_k_and_a_click_on_the_picture_play_and_pause() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    assert!(!viewer.is_playing());

    frame(&ctx, &mut viewer, press(Key::Space));
    assert!(viewer.is_playing());
    frame(&ctx, &mut viewer, press(Key::K));
    assert!(!viewer.is_playing());

    let middle = named_rect(&ctx, &mut viewer, "Play").center();
    click_named(&ctx, &mut viewer, "Play");
    assert!(viewer.is_playing(), "the play button plays");
    click_named(&ctx, &mut viewer, "Pause");
    assert!(!viewer.is_playing());
    let _ = middle;

    let picture = screen_camera::screen().center();
    click_at(&ctx, &mut viewer, picture);
    assert!(viewer.is_playing(), "a click on the picture plays");
    click_at(&ctx, &mut viewer, picture);
    assert!(!viewer.is_playing(), "and pauses");
}

#[test]
fn the_arrows_jump_by_the_seek_step_and_home_and_end_go_to_the_ends() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    viewer.set_seek_step(1);

    frame(&ctx, &mut viewer, press(Key::ArrowRight));
    assert!(near(viewer.position(), seconds(1.0), 0.01));
    frame(&ctx, &mut viewer, press(Key::ArrowRight));
    assert!(near(viewer.position(), seconds(2.0), 0.01));
    frame(&ctx, &mut viewer, press(Key::ArrowLeft));
    assert!(near(viewer.position(), seconds(1.0), 0.01));
    frame(&ctx, &mut viewer, press(Key::ArrowLeft));
    frame(&ctx, &mut viewer, press(Key::ArrowLeft));
    assert_eq!(viewer.position(), Ticks(0), "it stops at the start");

    frame(&ctx, &mut viewer, press(Key::End));
    assert!(near(viewer.position(), viewer.duration(), 0.001));
    frame(&ctx, &mut viewer, press(Key::ArrowRight));
    assert!(
        near(viewer.position(), viewer.duration(), 0.001),
        "and at the end"
    );
    frame(&ctx, &mut viewer, press(Key::Home));
    assert_eq!(viewer.position(), Ticks(0));

    viewer.set_seek_step(5);
    frame(&ctx, &mut viewer, press(Key::ArrowRight));
    assert!(
        near(viewer.position(), viewer.duration(), 0.001),
        "5 s is past 4"
    );
}

#[test]
fn comma_and_period_step_one_frame_and_pause() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    wait_for(&ctx, &mut viewer, "the frame table", |viewer| {
        viewer
            .file
            .as_ref()
            .is_some_and(|file| file.frames.is_some())
    });
    viewer.seek(seconds(1.0));
    viewer.play();

    frame(&ctx, &mut viewer, press(Key::Period));
    assert!(!viewer.is_playing(), "a frame step pauses");
    let after = viewer.position();
    frame(&ctx, &mut viewer, press(Key::Period));
    let frame_length = 1.0 / 30.0;
    assert!(
        near(
            viewer.position(),
            Ticks(after.0 + seconds(frame_length).0),
            0.002
        ),
        "{} then {}",
        after.as_seconds(),
        viewer.position().as_seconds()
    );
    frame(&ctx, &mut viewer, press(Key::Comma));
    assert!(near(viewer.position(), after, 0.002));
}

#[test]
fn up_down_and_m_change_the_volume_and_it_is_remembered() {
    let ctx = context();
    let folder = temporary_folder("volume");
    let mut viewer = Viewer::new(&ctx, false);
    viewer.use_storage(&ctx, folder.clone());
    viewer.open(&ctx, video());
    assert_eq!(viewer.volume(), 100);

    frame(&ctx, &mut viewer, press(Key::ArrowDown));
    frame(&ctx, &mut viewer, press(Key::ArrowDown));
    assert_eq!(viewer.volume(), 90);
    frame(&ctx, &mut viewer, press(Key::ArrowUp));
    assert_eq!(viewer.volume(), 95);
    for _ in 0..3 {
        frame(&ctx, &mut viewer, press(Key::ArrowUp));
    }
    assert_eq!(viewer.volume(), 100, "it stops at full");

    frame(&ctx, &mut viewer, press(Key::M));
    assert!(viewer.is_muted());
    let spans = viewer.heard();
    assert_eq!(spans[0].sound.volume, 0, "muted is silent");

    let saved = Settings::load(&folder).viewer;
    assert!(saved.muted);
    assert_eq!(saved.volume, 100);
    let mut again = Viewer::new(&ctx, false);
    again.use_storage(&ctx, folder);
    assert!(again.is_muted(), "a new player starts muted too");
}

#[test]
fn every_speed_sets_the_clip_and_the_playhead_keeps_to_it() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    for (index, speed) in SPEEDS.iter().enumerate() {
        viewer.set_speed(index);
        let clip = viewer.file.as_ref().unwrap().clip();
        assert!((clip.speed.as_factor() - speed).abs() < 1e-9, "{speed}");
        assert!(clip.speed.keep_pitch, "the voice keeps its pitch");
    }

    for (index, speed) in [(0, 0.25), (6, 2.0)] {
        viewer.set_speed(index);
        viewer.seek(Ticks(0));
        viewer.play();
        wait_for(&ctx, &mut viewer, "the clock to start", |viewer| {
            viewer.position() > Ticks(0)
        });
        let (from, started) = (viewer.position(), Instant::now());
        while started.elapsed() < Duration::from_millis(800) {
            frame(&ctx, &mut viewer, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        let moved = viewer.position().as_seconds() - from.as_seconds();
        let expected = started.elapsed().as_secs_f64() * speed;
        assert!(
            (moved - expected).abs() < 0.12 * speed.max(1.0),
            "at {speed}x it moved {moved:.3} s in the file, expected {expected:.3}"
        );
        viewer.pause();
    }
}

#[test]
fn the_speed_menu_shows_the_speed_on_its_button_and_is_never_saved() {
    let ctx = context();
    let folder = temporary_folder("speed");
    let mut viewer = Viewer::new(&ctx, false);
    viewer.use_storage(&ctx, folder.clone());
    viewer.open(&ctx, video());

    click_named(&ctx, &mut viewer, "1x");
    click_named(&ctx, &mut viewer, "1.5x");
    assert_eq!(viewer.speed(), 1.5);
    assert!(
        named(&ctx, &mut viewer)
            .iter()
            .any(|(label, ..)| label == "1.5x")
    );

    viewer.closing(None);
    let saved = std::fs::read_to_string(folder.join("settings.json")).unwrap();
    assert!(!saved.contains("1.5"), "the speed isn't saved: {saved}");
    viewer.open(&ctx, video());
    assert_eq!(viewer.speed(), 1.5, "it lasts from file to file");
    let clip = viewer.file.as_ref().unwrap().clip();
    assert!((clip.speed.as_factor() - 1.5).abs() < 1e-9);
    let mut fresh = Viewer::new(&ctx, false);
    fresh.use_storage(&ctx, folder);
    assert_eq!(fresh.speed(), 1.0, "a new window starts at 1x");
}

#[test]
fn loop_starts_again_at_the_end_and_without_it_playback_rests_on_the_last_picture() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    viewer.seek(seconds(3.6));
    viewer.play();
    wait_for(&ctx, &mut viewer, "the end", |viewer| !viewer.is_playing());
    assert!(near(viewer.position(), viewer.duration(), 0.05));
    assert!(viewer.picture.is_some(), "the last picture stays");

    click_named(&ctx, &mut viewer, "Loop");
    assert!(viewer.is_looping());
    viewer.seek(seconds(3.6));
    viewer.play();
    let started = Instant::now();
    let mut went_round = false;
    while started.elapsed() < Duration::from_secs(3) {
        frame(&ctx, &mut viewer, vec![]);
        if viewer.position() < seconds(1.0) && viewer.is_playing() {
            went_round = true;
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(went_round, "it started again from the start");
}

#[test]
fn f_and_escape_go_in_and_out_of_full_screen_where_the_bars_hide_when_the_mouse_rests() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    frame(&ctx, &mut viewer, press(Key::F));
    assert!(viewer.is_full_screen());
    assert!(viewer.bars_shown(&ctx), "the bars show at first");
    viewer.last_pointer_move = Instant::now() - BARS_LINGER - Duration::from_millis(10);
    assert!(!viewer.bars_shown(&ctx), "and hide once the mouse rests");
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(300.0, 300.0))],
    );
    assert!(viewer.bars_shown(&ctx), "moving it brings them back");

    frame(&ctx, &mut viewer, press(Key::Escape));
    assert!(!viewer.is_full_screen());
    viewer.last_pointer_move = Instant::now() - BARS_LINGER - Duration::from_millis(10);
    assert!(viewer.bars_shown(&ctx), "in a window they always show");
}

#[test]
fn the_seek_step_is_saved_kept_within_1_to_60_and_used_by_the_arrows() {
    let ctx = context();
    let folder = temporary_folder("seek-step");
    let mut viewer = Viewer::new(&ctx, false);
    viewer.use_storage(&ctx, folder.clone());
    assert_eq!(viewer.seek_step(), 5);

    viewer.set_seek_step(0);
    assert_eq!(viewer.seek_step(), 1);
    viewer.set_seek_step(90);
    assert_eq!(viewer.seek_step(), 60);
    viewer.set_seek_step(2);
    assert_eq!(Settings::load(&folder).viewer.seek_seconds, 2);

    let mut again = Viewer::new(&ctx, false);
    again.use_storage(&ctx, folder);
    assert_eq!(again.seek_step(), 2);
    again.open(&ctx, video());
    again.pause();
    again.seek(Ticks(0));
    frame(&ctx, &mut again, press(Key::ArrowRight));
    assert!(near(again.position(), seconds(2.0), 0.01));
}

#[test]
fn the_player_keeps_its_own_window_place_apart_from_the_editors() {
    let ctx = context();
    let folder = temporary_folder("place");
    let editor_place = WindowPlace {
        left: 0,
        top: 0,
        right: 1280,
        bottom: 800,
        maximized: true,
    };
    let mut settings = Settings::load(&folder);
    settings.window = Some(editor_place);
    settings.save(&folder).unwrap();

    let mut viewer = Viewer::new(&ctx, false);
    viewer.use_storage(&ctx, folder.clone());
    assert_eq!(
        viewer.saved_window(),
        None,
        "the first time it opens centred"
    );
    let place = WindowPlace {
        left: 100,
        top: 120,
        right: 1380,
        bottom: 840,
        maximized: false,
    };
    viewer.closing(Some(place));

    let saved = Settings::load(&folder);
    assert_eq!(saved.viewer.window, Some(place));
    assert_eq!(
        saved.window,
        Some(editor_place),
        "the editor's place is untouched"
    );
    let mut again = Viewer::new(&ctx, false);
    again.use_storage(&ctx, folder);
    assert_eq!(again.saved_window(), Some(place));
}

#[test]
fn a_song_shows_its_cover_and_one_without_shows_the_mark() {
    let ctx = context();
    let mut viewer = opened(&ctx, &make_song("cover", true));
    wait_for(&ctx, &mut viewer, "the cover", |viewer| {
        viewer.file.as_ref().is_some_and(|file| file.cover_read)
    });
    let cover = viewer
        .file
        .as_ref()
        .unwrap()
        .cover
        .as_ref()
        .expect("a cover");
    assert_eq!(cover.size(), [300, 300]);
    assert!(
        near(viewer.duration(), seconds(3.0), 0.1),
        "a cover doesn't shorten the song: {}",
        viewer.duration().as_seconds()
    );

    let mut viewer = opened(&ctx, &make_song("no-cover", false));
    wait_for(&ctx, &mut viewer, "the cover's read", |viewer| {
        viewer.file.as_ref().is_some_and(|file| file.cover_read)
    });
    assert!(viewer.file.as_ref().unwrap().cover.is_none());
    frame(&ctx, &mut viewer, vec![]);
    assert!(viewer.mark.is_some(), "the mark stands in");
    assert_eq!(file_details(viewer.file.as_ref().unwrap())[0], "MP3");
    assert_eq!(
        file_details(viewer.file.as_ref().unwrap()).len(),
        2,
        "no picture size"
    );
}

#[test]
fn a_song_plays_and_its_position_moves() {
    let ctx = context();
    let mut viewer = opened(&ctx, &make_song("plays", false));
    viewer.play();
    wait_for(&ctx, &mut viewer, "the song to move", |viewer| {
        viewer.position() > seconds(0.2)
    });
    assert!(near(viewer.duration(), seconds(3.0), 0.1));
}

#[test]
fn every_sound_track_is_heard_together() {
    let folder = temporary_folder("tracks");
    let path = folder.join("two-tracks.mkv");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc2=s=160x90:r=30:d=2"])
        .args(["-f", "lavfi", "-i", "sine=frequency=300:duration=2"])
        .args(["-f", "lavfi", "-i", "sine=frequency=600:duration=2"])
        .args(["-map", "0", "-map", "1", "-map", "2"])
        .args(["-c:v", "libopenh264", "-c:a", "aac"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    let ctx = context();
    let viewer = opened(&ctx, &path);
    let spans = viewer.heard();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].sound.only_track, None, "no track is left out");
    assert_eq!(spans[0].bus, Bus::All(0));
}

#[test]
fn the_top_bar_keeps_its_buttons_greyed_until_they_work() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let nodes = named(&ctx, &mut viewer);
    for name in [
        "Close window",
        "Maximise window",
        "Minimise window",
        "Settings",
    ] {
        assert!(
            nodes.iter().any(|(label, _, off)| label == name && !off),
            "{name} works"
        );
    }
    click_named(&ctx, &mut viewer, "Settings");
    assert!(viewer.settings_open);
    frame(&ctx, &mut viewer, press(Key::Escape));
    assert!(!viewer.settings_open, "Escape closes Settings");
}

fn wheel(at: egui::Pos2, notches: f32) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(at),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, notches),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: Modifiers::NONE,
        },
    ]
}

fn ctrl(key: Key) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::CTRL | Modifiers::COMMAND,
    }]
}

/// Every command a frame sent to the window.
fn commands(output: &egui::FullOutput) -> Vec<egui::ViewportCommand> {
    output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|viewport| viewport.commands.clone())
        .unwrap_or_default()
}

/// Clicks a control by name and gathers what the window was told to do meanwhile.
fn click_named_for_commands(
    ctx: &egui::Context,
    viewer: &mut Viewer,
    name: &str,
) -> Vec<egui::ViewportCommand> {
    let at = named_rect(ctx, viewer, name).center();
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    let mut sent = Vec::new();
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![button(true)],
        vec![button(false)],
    ] {
        sent.extend(commands(&frame(ctx, viewer, events)));
    }
    sent
}

#[test]
fn the_wheel_zooms_toward_the_pointer_and_a_double_click_or_0_fits() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    frame(&ctx, &mut viewer, vec![]);
    let fit = viewer.zoom_percent(&ctx);
    let view = viewer.view;
    let corner = view.min + vec2(40.0, 40.0);

    frame(&ctx, &mut viewer, wheel(corner, 1.0));
    assert!(
        (viewer.zoom_percent(&ctx) - fit * zoom::STEP).abs() < 0.5,
        "{} from {fit}",
        viewer.zoom_percent(&ctx)
    );
    frame(&ctx, &mut viewer, wheel(corner, -3.0));
    assert!(viewer.zoom.is_fit(), "it never goes below fit");

    frame(&ctx, &mut viewer, wheel(view.center(), 2.0));
    assert!(!viewer.zoom.is_fit());
    click_at(&ctx, &mut viewer, view.center());
    assert!(!viewer.is_playing(), "a click while zoomed doesn't play");
    frame(&ctx, &mut viewer, press(Key::Num0));
    assert!(viewer.zoom.is_fit(), "0 fits");

    frame(&ctx, &mut viewer, ctrl(Key::Equals));
    assert!(!viewer.zoom.is_fit(), "Ctrl+= zooms in");
    frame(&ctx, &mut viewer, ctrl(Key::Minus));
    assert!(viewer.zoom.is_fit(), "Ctrl+- zooms out");

    frame(&ctx, &mut viewer, wheel(view.center(), 2.0));
    let double = |pressed| egui::Event::PointerButton {
        pos: view.center(),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    for pressed in [true, false, true, false] {
        frame(&ctx, &mut viewer, vec![double(pressed)]);
    }
    frame(&ctx, &mut viewer, vec![]);
    assert!(viewer.zoom.is_fit(), "a double-click fits");
}

#[test]
fn dragging_pans_only_while_zoomed() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    frame(&ctx, &mut viewer, vec![]);
    let view = viewer.view;
    frame(&ctx, &mut viewer, wheel(view.center(), 3.0));
    let video_size = viewer.video_size().unwrap();
    let before = viewer.zoom.picture_rect(view, video_size);

    let from = view.center();
    let to = from + vec2(-60.0, -30.0);
    let button = |at, pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    frame(&ctx, &mut viewer, vec![egui::Event::PointerMoved(from)]);
    frame(&ctx, &mut viewer, vec![button(from, true)]);
    for step in 1..=6 {
        let at = from + (to - from) * (step as f32 / 6.0);
        frame(&ctx, &mut viewer, vec![egui::Event::PointerMoved(at)]);
    }
    frame(&ctx, &mut viewer, vec![button(to, false)]);
    let after = viewer.zoom.picture_rect(view, video_size);
    let moved = after.min - before.min;
    assert!(moved.x < -20.0 && moved.y < -5.0, "it moved by {moved:?}");
    assert!(!viewer.is_playing(), "a pan doesn't play");
}

#[test]
fn zooming_in_decodes_the_video_at_full_size_and_fit_goes_back() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    frame(&ctx, &mut viewer, vec![]);
    assert!(!viewer.is_sharp());
    let view = viewer.view;
    frame(&ctx, &mut viewer, wheel(view.center(), 2.0));
    frame(&ctx, &mut viewer, vec![]);
    assert!(viewer.is_sharp());
    frame(&ctx, &mut viewer, press(Key::Num0));
    frame(&ctx, &mut viewer, vec![]);
    assert!(!viewer.is_sharp());
}

#[test]
fn each_new_file_starts_at_fit() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    frame(&ctx, &mut viewer, vec![]);
    let view = viewer.view;
    frame(&ctx, &mut viewer, wheel(view.center(), 2.0));
    assert!(!viewer.zoom.is_fit());
    viewer.open(&ctx, video());
    assert!(viewer.zoom.is_fit());
}

#[test]
fn save_picture_writes_the_picture_beside_the_video_named_for_its_time() {
    let ctx = context();
    let folder = temporary_folder("save-picture");
    let copy = folder.join("Clip.mp4");
    std::fs::copy(video(), &copy).unwrap();
    let mut viewer = opened(&ctx, &copy);
    viewer.seek(seconds(1.0));
    frame(&ctx, &mut viewer, vec![]);

    click_named(&ctx, &mut viewer, "Save picture");
    wait_for(&ctx, &mut viewer, "the picture to save", |viewer| {
        viewer.saved_picture.is_some()
    });
    let saved = folder.join("Clip 0-01.00.png");
    assert_eq!(viewer.saved_picture.as_ref().unwrap().0, saved);
    let picture = bareclip_media::read_still(&saved).unwrap();
    assert_eq!(
        (picture.width, picture.height),
        (320, 180),
        "the video's real size"
    );
    assert!(
        named(&ctx, &mut viewer)
            .iter()
            .any(|(label, ..)| label == "Show in folder")
    );

    click_named(&ctx, &mut viewer, "Close note");
    assert!(viewer.saved_picture.is_none());
    click_named(&ctx, &mut viewer, "Save picture");
    wait_for(&ctx, &mut viewer, "the second picture", |viewer| {
        viewer.saved_picture.is_some()
    });
    assert_eq!(
        viewer.saved_picture.as_ref().unwrap().0,
        folder.join("Clip 0-01.00 (2).png"),
        "a second save doesn't overwrite the first"
    );
}

#[test]
fn a_song_has_no_save_picture_or_zoom() {
    let ctx = context();
    let mut viewer = opened(&ctx, &make_song("no-zoom", false));
    let nodes = named(&ctx, &mut viewer);
    for name in ["Save picture", "Zoom in", "Zoom out", "Fit"] {
        assert!(
            !nodes.iter().any(|(label, _, off)| label == name && !off),
            "{name} is hidden"
        );
    }
}

#[test]
fn mini_view_shrinks_the_window_above_the_others_and_leaving_puts_it_back() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let sent = click_named_for_commands(&ctx, &mut viewer, "Mini view");
    assert!(viewer.is_mini());
    assert!(sent.contains(&egui::ViewportCommand::WindowLevel(
        egui::WindowLevel::AlwaysOnTop
    )));
    assert!(sent.contains(&egui::ViewportCommand::InnerSize(mini::MINI_SIZE)));

    // Only the picture shows, until the mouse is over it.
    frame(&ctx, &mut viewer, vec![egui::Event::PointerGone]);
    let nodes = named(&ctx, &mut viewer);
    assert!(
        !nodes.iter().any(|(label, ..)| label == "Settings"),
        "no bars"
    );
    assert!(!nodes.iter().any(|(label, ..)| label == "Leave mini view"));
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(200.0, 100.0))],
    );
    let nodes = named(&ctx, &mut viewer);
    for name in ["Play", "Seek", "Leave mini view"] {
        assert!(
            nodes.iter().any(|(label, ..)| label == name),
            "{name} on hover"
        );
    }

    let sent = click_named_for_commands(&ctx, &mut viewer, "Leave mini view");
    assert!(!viewer.is_mini());
    assert!(sent.contains(&egui::ViewportCommand::WindowLevel(
        egui::WindowLevel::Normal
    )));
    let screen = screen_camera::screen().size();
    assert!(
        sent.contains(&egui::ViewportCommand::InnerSize(screen)),
        "the size it had comes back: {sent:?}"
    );

    viewer.enter_mini_view(&ctx);
    frame(&ctx, &mut viewer, press(Key::Escape));
    assert!(!viewer.is_mini(), "Escape leaves too");
}

/// A 30 second video, small and slow so it is quick to make, long enough to resume.
fn long_video() -> &'static Path {
    static VIDEO: OnceLock<PathBuf> = OnceLock::new();
    VIDEO.get_or_init(|| {
        let folder = std::env::temp_dir().join("bareclip-viewer-tests");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("long-{}.mp4", std::process::id()));
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc2=s=160x90:r=10:d=30"])
            .args(["-c:v", "libopenh264", "-g", "10"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        path
    })
}

/// A folder holding copies of the test video under these names.
fn folder_of_videos(name: &str, files: &[&str]) -> PathBuf {
    let folder = temporary_folder(name);
    for file in files {
        std::fs::copy(video(), folder.join(file)).unwrap();
    }
    folder
}

/// Runs frames until the folder's files have been read.
fn wait_for_folder(ctx: &egui::Context, viewer: &mut Viewer) {
    wait_for(ctx, viewer, "the folder", |viewer| {
        viewer.folder_reading.is_none()
    });
}

#[test]
fn page_up_and_down_go_through_the_folder_in_name_order_and_stop_at_the_ends() {
    let ctx = context();
    let folder = folder_of_videos(
        "paging",
        &["clip 10.mp4", "clip 2.mp4", "Clip 1.mkv", "notes.txt"],
    );
    std::fs::write(folder.join("notes.txt"), b"not a video").unwrap();
    let mut viewer = opened(&ctx, &folder.join("clip 2.mp4"));
    wait_for_folder(&ctx, &mut viewer);

    frame(&ctx, &mut viewer, press(Key::PageDown));
    assert_eq!(
        viewer.current_path(),
        Some(folder.join("clip 10.mp4").as_path())
    );
    wait_for_folder(&ctx, &mut viewer);
    frame(&ctx, &mut viewer, press(Key::PageDown));
    assert_eq!(
        viewer.current_path(),
        Some(folder.join("clip 10.mp4").as_path()),
        "it stops at the last"
    );
    frame(&ctx, &mut viewer, press(Key::PageUp));
    wait_for_folder(&ctx, &mut viewer);
    frame(&ctx, &mut viewer, press(Key::PageUp));
    assert_eq!(
        viewer.current_path(),
        Some(folder.join("Clip 1.mkv").as_path())
    );
    wait_for_folder(&ctx, &mut viewer);
    frame(&ctx, &mut viewer, press(Key::PageUp));
    assert_eq!(
        viewer.current_path(),
        Some(folder.join("Clip 1.mkv").as_path()),
        "and at the first"
    );
}

#[test]
fn the_edge_arrows_show_on_hover_and_hide_at_the_ends() {
    let ctx = context();
    let folder = folder_of_videos("arrows", &["a.mp4", "b.mp4", "c.mp4"]);
    let mut viewer = opened(&ctx, &folder.join("b.mp4"));
    wait_for_folder(&ctx, &mut viewer);
    frame(&ctx, &mut viewer, vec![egui::Event::PointerGone]);
    let nodes = named(&ctx, &mut viewer);
    assert!(
        !nodes.iter().any(|(label, ..)| label == "Next file"),
        "hidden at rest"
    );

    let over = screen_camera::screen().center();
    frame(&ctx, &mut viewer, vec![egui::Event::PointerMoved(over)]);
    let nodes = named(&ctx, &mut viewer);
    assert!(nodes.iter().any(|(label, ..)| label == "Next file"));
    assert!(nodes.iter().any(|(label, ..)| label == "Previous file"));

    click_named(&ctx, &mut viewer, "Next file");
    assert_eq!(viewer.current_path(), Some(folder.join("c.mp4").as_path()));
    wait_for_folder(&ctx, &mut viewer);
    frame(&ctx, &mut viewer, vec![egui::Event::PointerMoved(over)]);
    let nodes = named(&ctx, &mut viewer);
    assert!(
        !nodes.iter().any(|(label, ..)| label == "Next file"),
        "none after the last"
    );
    assert!(nodes.iter().any(|(label, ..)| label == "Previous file"));
}

#[test]
fn a_file_that_cant_be_played_still_moves_on() {
    let ctx = context();
    let folder = folder_of_videos("broken-next", &["b.mp4"]);
    std::fs::write(folder.join("a.mp4"), b"broken").unwrap();
    let mut viewer = Viewer::new(&ctx, false);
    viewer.open(&ctx, &folder.join("a.mp4"));
    assert!(viewer.failed.is_some());
    wait_for_folder(&ctx, &mut viewer);
    frame(&ctx, &mut viewer, press(Key::PageDown));
    assert!(viewer.file.is_some(), "the next one plays");
}

#[test]
fn a_file_closed_partway_picks_up_there_with_start_over() {
    let ctx = context();
    let storage = temporary_folder("resume-storage");
    let mut viewer = Viewer::new(&ctx, false);
    viewer.use_storage(&ctx, storage.clone());
    viewer.open(&ctx, long_video());
    viewer.pause();
    viewer.seek(seconds(15.0));
    viewer.closing(None);

    let mut again = Viewer::new(&ctx, false);
    again.use_storage(&ctx, storage.clone());
    again.open(&ctx, long_video());
    assert!(near(again.position(), seconds(15.0), 0.01), "it picks up");
    assert!(again.is_playing());
    assert!(
        named(&ctx, &mut again)
            .iter()
            .any(|(label, ..)| label == "Start over")
    );
    click_named(&ctx, &mut again, "Start over");
    assert!(again.position() < seconds(1.0));
    assert!(again.resumed.is_none());

    // Switching files keeps the spot too.
    again.pause();
    again.seek(seconds(18.0));
    again.open(&ctx, video());
    again.open(&ctx, long_video());
    assert!(near(again.position(), seconds(18.0), 0.01));

    // Watched near the end, it starts at the beginning next time.
    again.pause();
    again.seek(seconds(25.0));
    again.closing(None);
    let mut third = Viewer::new(&ctx, false);
    third.use_storage(&ctx, storage);
    third.open(&ctx, long_video());
    assert!(third.position() < seconds(1.0));
    assert!(third.resumed.is_none());
}

#[test]
fn with_resume_off_files_start_from_the_start() {
    let ctx = context();
    let storage = temporary_folder("resume-off");
    let mut viewer = Viewer::new(&ctx, false);
    viewer.use_storage(&ctx, storage.clone());
    viewer.set_resume(false);
    viewer.open(&ctx, long_video());
    viewer.pause();
    viewer.seek(seconds(15.0));
    viewer.closing(None);

    let mut again = Viewer::new(&ctx, false);
    again.use_storage(&ctx, storage.clone());
    assert!(
        !Settings::load(&storage).viewer.resume,
        "the switch is saved"
    );
    again.open(&ctx, long_video());
    assert!(again.position() < seconds(1.0));
}

#[test]
fn a_spot_is_kept_every_few_seconds_while_playing() {
    let ctx = context();
    let storage = temporary_folder("resume-playing");
    let mut viewer = Viewer::new(&ctx, false);
    viewer.use_storage(&ctx, storage.clone());
    viewer.open(&ctx, long_video());
    viewer.seek(seconds(12.0));
    viewer.play();
    viewer.spot_kept = Instant::now() - Duration::from_secs(6);
    frame(&ctx, &mut viewer, vec![]);
    // Nothing closed, as if the app had crashed.
    let spots = resume::ResumeSpots::load(&storage);
    let at = spots.spot_for(long_video()).expect("the spot was written");
    assert!(near(at, seconds(12.0), 0.5));
}

/// Moves a file into a folder of the tests' own, in place of the Recycle Bin.
fn test_trash(path: &Path) -> Result<(), String> {
    let bin = std::env::temp_dir()
        .join("bareclip-viewer-tests")
        .join(format!("bin-{}", std::process::id()));
    std::fs::create_dir_all(&bin).map_err(|error| error.to_string())?;
    std::fs::rename(path, bin.join(path.file_name().unwrap())).map_err(|error| error.to_string())
}

fn type_text(ctx: &egui::Context, viewer: &mut Viewer, field: &str, typed: &str) {
    click_named(ctx, viewer, field);
    let select_all = vec![egui::Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::CTRL | Modifiers::COMMAND,
    }];
    frame(ctx, viewer, select_all);
    frame(ctx, viewer, vec![egui::Event::Text(typed.to_owned())]);
    frame(ctx, viewer, press(Key::Enter));
    frame(ctx, viewer, vec![]);
}

fn wait_for_details(ctx: &egui::Context, viewer: &mut Viewer) {
    wait_for(ctx, viewer, "the details", |viewer| {
        viewer.info.as_ref().is_some_and(|info| info.has_details())
    });
}

#[test]
fn renaming_and_deleting_while_playing_both_work_because_the_file_is_let_go() {
    let ctx = context();
    let folder = folder_of_videos("let-go", &["clip.mp4", "other.mp4"]);
    let mut viewer = opened(&ctx, &folder.join("clip.mp4"));
    viewer.trash_file = test_trash;
    wait_for_folder(&ctx, &mut viewer);
    viewer.seek(seconds(1.0));
    viewer.play();
    frame(&ctx, &mut viewer, vec![]);

    viewer.rename_file(&ctx, "renamed clip");
    let renamed = folder.join("renamed clip.mp4");
    assert!(renamed.exists() && !folder.join("clip.mp4").exists());
    assert_eq!(viewer.current_path(), Some(renamed.as_path()));
    assert!(viewer.is_playing(), "it carries on playing");
    assert!(
        near(viewer.position(), seconds(1.0), 0.3),
        "from the same spot"
    );

    viewer.delete_file(&ctx);
    assert!(!renamed.exists(), "it is gone from the folder");
    assert_eq!(
        viewer.current_path(),
        Some(folder.join("other.mp4").as_path()),
        "the next file shows"
    );
}

#[test]
fn delete_asks_first_and_shows_the_previous_file_after_the_last_or_closes() {
    let ctx = context();
    let folder = folder_of_videos("delete-order", &["a.mp4", "b.mp4"]);
    let mut viewer = opened(&ctx, &folder.join("b.mp4"));
    viewer.trash_file = test_trash;
    wait_for_folder(&ctx, &mut viewer);

    click_named(&ctx, &mut viewer, "Delete");
    assert!(viewer.confirm_delete, "it asks first");
    click_named(&ctx, &mut viewer, "Cancel");
    assert!(folder.join("b.mp4").exists());
    frame(&ctx, &mut viewer, press(Key::Delete));
    assert!(viewer.confirm_delete, "the Delete key asks too");
    click_named(&ctx, &mut viewer, "Move to Recycle Bin");
    assert!(!folder.join("b.mp4").exists());
    assert_eq!(viewer.current_path(), Some(folder.join("a.mp4").as_path()));

    wait_for_folder(&ctx, &mut viewer);
    viewer.ask_to_delete();
    let at = named_rect(&ctx, &mut viewer, "Move to Recycle Bin").center();
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    let mut sent = Vec::new();
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![button(true)],
        vec![button(false)],
    ] {
        sent.extend(commands(&frame(&ctx, &mut viewer, events)));
    }
    assert!(!folder.join("a.mp4").exists());
    assert!(
        sent.contains(&egui::ViewportCommand::Close),
        "nothing is left, so the window closes"
    );
}

#[test]
fn the_info_panel_renames_from_its_name_box_and_refuses_a_clash() {
    let ctx = context();
    let folder = folder_of_videos("info-rename", &["clip.mp4", "taken.mp4"]);
    let mut viewer = opened(&ctx, &folder.join("clip.mp4"));
    click_named(&ctx, &mut viewer, "Info");
    assert!(viewer.is_info_open());
    wait_for_details(&ctx, &mut viewer);

    type_text(&ctx, &mut viewer, "File name", "Holiday");
    assert_eq!(
        viewer.current_path(),
        Some(folder.join("Holiday.mp4").as_path())
    );

    type_text(&ctx, &mut viewer, "File name", "taken");
    assert_eq!(
        viewer.current_path(),
        Some(folder.join("Holiday.mp4").as_path()),
        "the old name stays"
    );
    let problem = viewer
        .info
        .as_ref()
        .unwrap()
        .problem
        .clone()
        .unwrap_or_default();
    assert!(problem.contains("already a file"), "{problem}");

    click_named(&ctx, &mut viewer, "File name");
    frame(&ctx, &mut viewer, vec![egui::Event::Text(" 2".to_owned())]);
    frame(&ctx, &mut viewer, press(Key::M));
    assert!(
        !viewer.is_muted(),
        "a letter typed into a name isn't a command"
    );
    frame(&ctx, &mut viewer, press(Key::Escape));
    for _ in 0..3 {
        frame(&ctx, &mut viewer, vec![]);
    }
    assert_eq!(
        viewer.current_path(),
        Some(folder.join("Holiday.mp4").as_path()),
        "Escape changes nothing"
    );

    type_text(&ctx, &mut viewer, "File name", "a:b");
    let problem = viewer
        .info
        .as_ref()
        .unwrap()
        .problem
        .clone()
        .unwrap_or_default();
    assert!(problem.contains(":"), "{problem}");

    viewer.open(&ctx, &folder.join("taken.mp4"));
    assert!(viewer.is_info_open(), "it stays open from file to file");
    click_named(&ctx, &mut viewer, "Close info");
    assert!(!viewer.is_info_open());
}

#[test]
fn windows_dates_are_changed_from_the_panel() {
    let ctx = context();
    let folder = folder_of_videos("info-dates", &["clip.mp4"]);
    let path = folder.join("clip.mp4");
    let mut viewer = opened(&ctx, &path);
    viewer.toggle_info();
    wait_for_details(&ctx, &mut viewer);

    type_text(&ctx, &mut viewer, "Created", "2021-03-04 05:06:07");
    type_text(&ctx, &mut viewer, "Modified", "2022-08-09 10:11:12");
    let (created, modified) = crate::file_ops::file_times(&path).unwrap();
    assert_eq!(
        crate::file_ops::local_parts(created).to_typed(),
        "2021-03-04 05:06:07"
    );
    assert_eq!(
        crate::file_ops::local_parts(modified).to_typed(),
        "2022-08-09 10:11:12"
    );

    type_text(&ctx, &mut viewer, "Created", "not a date");
    let problem = viewer
        .info
        .as_ref()
        .unwrap()
        .problem
        .clone()
        .unwrap_or_default();
    assert!(problem.contains("like 2025-09-14"), "{problem}");
}

#[test]
fn the_artist_and_creation_time_are_written_into_the_file_keeping_its_dates() {
    let ctx = context();
    let folder = folder_of_videos("info-tags", &["clip.mp4"]);
    let path = folder.join("clip.mp4");
    let created = crate::file_ops::from_local(DateTime::parse("2020-01-01 10:00").unwrap());
    let modified = crate::file_ops::from_local(DateTime::parse("2020-02-02 11:00").unwrap());
    crate::file_ops::set_file_times(&path, created, modified).unwrap();
    let mut viewer = opened(&ctx, &path);
    viewer.toggle_info();
    wait_for_details(&ctx, &mut viewer);

    type_text(&ctx, &mut viewer, "Artist", "Somebody");
    wait_for(&ctx, &mut viewer, "the rewrite", |viewer| {
        !viewer.is_rewriting()
    });
    // The file is read again after it changes, and the boxes come back once it has been.
    frame(&ctx, &mut viewer, vec![]);
    wait_for_details(&ctx, &mut viewer);
    type_text(&ctx, &mut viewer, "Made", "2019-05-06 07:08:09");
    wait_for(&ctx, &mut viewer, "the second rewrite", |viewer| {
        !viewer.is_rewriting()
    });

    let details = bareclip_media::read_details(&path).unwrap();
    assert_eq!(details.tag("artist"), Some("Somebody"));
    let made = details
        .tag("creation_time")
        .and_then(info::read_creation_time);
    assert_eq!(
        made.map(|made| crate::file_ops::local_parts(made).to_typed()),
        Some("2019-05-06 07:08:09".to_owned())
    );
    let (kept_created, kept_modified) = crate::file_ops::file_times(&path).unwrap();
    assert_eq!(
        crate::file_ops::local_parts(kept_created).to_typed(),
        "2020-01-01 10:00:00"
    );
    assert_eq!(
        crate::file_ops::local_parts(kept_modified).to_typed(),
        "2020-02-02 11:00:00"
    );
    assert!(viewer.file.is_some(), "it plays again afterwards");
    let leftovers: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains("bareclip-copy")
        })
        .collect();
    assert!(leftovers.is_empty(), "no copy is left behind");
}

#[test]
fn a_kind_of_file_whose_tags_cant_be_written_shows_them_read_only() {
    let ctx = context();
    let folder = temporary_folder("info-avi");
    let path = folder.join("clip.avi");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=160x90:r=10:d=1",
            "-c:v",
            "mpeg4",
        ])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    let mut viewer = opened(&ctx, &path);
    viewer.toggle_info();
    wait_for_details(&ctx, &mut viewer);
    let nodes = named(&ctx, &mut viewer);
    assert!(
        nodes.iter().any(|(label, ..)| label == "File name"),
        "it can still be renamed"
    );
    assert!(!nodes.iter().any(|(label, ..)| label == "Artist"));
    assert!(!nodes.iter().any(|(label, ..)| label == "Made"));
}

/// Which frame of `video()` each frame of a file is, read from the binary pattern across it.
fn frame_numbers(path: &Path) -> Vec<u32> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-vf", "scale=320:180,format=gray", "-f", "rawvideo", "-"])
        .output()
        .unwrap();
    assert!(output.status.success());
    output
        .stdout
        .chunks(320 * 180)
        .map(|frame| {
            (0..16)
                .filter(|bit| frame[90 * 320 + bit * 20 + 10] > 128)
                .map(|bit| 1 << bit)
                .sum()
        })
        .collect()
}

/// The time partway through a frame of `video()`.
fn during_frame(number: u32) -> Ticks {
    seconds((f64::from(number) + 0.5) / 30.0)
}

fn trimmer_on(ctx: &egui::Context, path: &Path) -> Viewer {
    let mut viewer = opened(ctx, path);
    wait_for(ctx, &mut viewer, "the frame table", |viewer| {
        viewer
            .file
            .as_ref()
            .is_some_and(|file| file.frames.is_some() || file.is_sound())
    });
    viewer.open_trimmer(ctx);
    viewer
}

fn wait_for_save(ctx: &egui::Context, viewer: &mut Viewer) -> PathBuf {
    wait_for(ctx, viewer, "the save", |viewer| !viewer.is_saving_trim());
    let trimmer = viewer.trim.as_ref().expect("the trimmer stays open");
    assert!(trimmer.problem.is_none(), "{:?}", trimmer.problem);
    trimmer.saved.as_ref().expect("it was saved").0.clone()
}

#[test]
fn the_handles_land_on_frames_never_cross_and_keep_at_least_one_frame() {
    let ctx = context();
    let mut viewer = trimmer_on(&ctx, video());
    let one = 1.0 / 30.0;

    viewer.set_trim_start(during_frame(17));
    viewer.set_trim_end(during_frame(82));
    let range = viewer.trim_range().unwrap();
    assert!(
        near(range.start, seconds(17.0 * one), 0.0005),
        "the start of frame 17"
    );
    assert!(
        near(range.end, seconds(83.0 * one), 0.0005),
        "frame 82 is the last kept"
    );

    viewer.set_trim_start(seconds(10.0));
    let range = viewer.trim_range().unwrap();
    assert!(range.start < range.end, "the start can't pass the end");
    assert!(
        range.end.0 - range.start.0 >= seconds(one).0 - 10,
        "at least one frame is kept"
    );
    viewer.set_trim_end(Ticks(0));
    let range = viewer.trim_range().unwrap();
    assert!(range.end > range.start);

    viewer.seek(during_frame(30));
    frame(&ctx, &mut viewer, press(Key::OpenBracket));
    viewer.seek(during_frame(60));
    frame(&ctx, &mut viewer, press(Key::CloseBracket));
    let range = viewer.trim_range().unwrap();
    assert!(
        near(range.start, seconds(30.0 * one), 0.0005),
        "[ puts the start at the playhead"
    );
    assert!(
        near(range.end, seconds(61.0 * one), 0.0005),
        "] puts the end after its frame"
    );

    frame(&ctx, &mut viewer, press(Key::Escape));
    assert!(!viewer.is_trimming(), "Escape leaves the trimmer");
    viewer.open_trimmer(&ctx);
    let range = viewer.trim_range().unwrap();
    assert_eq!(range.start, Ticks(0), "and forgets the handles");
}

#[test]
fn playing_in_the_trimmer_runs_between_the_handles_and_stops_at_the_end_one() {
    let ctx = context();
    let mut viewer = trimmer_on(&ctx, video());
    viewer.set_trim_start(seconds(1.0));
    viewer.set_trim_end(seconds(1.5));
    viewer.seek(seconds(3.0));
    viewer.toggle_playback();
    assert!(
        viewer.position() < seconds(1.1),
        "outside the handles, play starts at the start"
    );
    wait_for(&ctx, &mut viewer, "the end handle", |viewer| {
        !viewer.is_playing()
    });
    let range = viewer.trim_range().unwrap();
    assert!(
        viewer.position() < range.end && viewer.position() > seconds(1.4),
        "stopped at {} with the end at {}",
        viewer.position().as_seconds(),
        range.end.as_seconds()
    );
}

#[test]
fn a_trim_is_saved_frame_exact_beside_the_original() {
    let ctx = context();
    let folder = folder_of_videos("trim-exact", &["numbers.mp4"]);
    let source = folder.join("numbers.mp4");
    let mut viewer = trimmer_on(&ctx, &source);
    viewer.set_trim_start(during_frame(17));
    viewer.set_trim_end(during_frame(82));
    click_named(&ctx, &mut viewer, "Save as copy");
    let saved = wait_for_save(&ctx, &mut viewer);
    assert_eq!(saved, folder.join("numbers (trimmed).mp4"));
    assert_eq!(frame_numbers(&saved), (17..=82).collect::<Vec<u32>>());
    assert!(viewer.is_trimming(), "the trimmer stays open");
    let summary = &viewer.trim.as_ref().unwrap().saved.as_ref().unwrap().1;
    assert!(!summary.is_empty(), "it says how it was saved");

    // A second copy doesn't overwrite the first.
    click_named(&ctx, &mut viewer, "Save as copy");
    assert_eq!(
        wait_for_save(&ctx, &mut viewer),
        folder.join("numbers (trimmed 2).mp4")
    );
    click_named(&ctx, &mut viewer, "Play");
    assert_eq!(
        viewer.current_path(),
        Some(folder.join("numbers (trimmed 2).mp4").as_path()),
        "Play opens the new file here"
    );
}

/// The same frame-exact save with NVIDIA's and the other graphics encoders out of reach, run
/// in a process of its own because the setting is for the whole process.
#[test]
fn a_trim_is_saved_frame_exact_without_a_graphics_encoder_too() {
    if std::env::var_os(bareclip_media::SKIP_ENCODERS_VARIABLE).is_some() {
        return;
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "viewer::tests::a_trim_is_saved_frame_exact_beside_the_original",
        ])
        .args(["--test-threads", "1"])
        .env(bareclip_media::SKIP_ENCODERS_VARIABLE, "nvenc,amf,qsv")
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn mp4_mkv_and_mov_stay_as_they_are_and_wmv_becomes_an_mp4() {
    let ctx = context();
    let folder = temporary_folder("trim-kinds");
    for (name, codec) in [
        ("clip.mp4", &["-c:v", "libopenh264"][..]),
        ("clip.mkv", &["-c:v", "libopenh264"][..]),
        ("clip.mov", &["-c:v", "libopenh264"][..]),
        ("clip.wmv", &["-c:v", "wmv2"][..]),
    ] {
        let path = folder.join(name);
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc2=s=160x90:r=30:d=2"])
            .args(codec)
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let mut viewer = trimmer_on(&ctx, &path);
        viewer.set_trim_start(seconds(0.5));
        viewer.set_trim_end(seconds(1.5));
        viewer.save_trim(&ctx, None);
        let saved = wait_for_save(&ctx, &mut viewer);
        let extension = saved.extension().unwrap().to_string_lossy().into_owned();
        let expected = if name.ends_with("wmv") {
            "mp4"
        } else {
            &name[5..]
        };
        assert_eq!(extension, expected, "{name}");
        let info = bareclip_media::probe(&saved).unwrap();
        assert_eq!(
            (info.video.width, info.video.height),
            (160, 90),
            "{name} has a picture"
        );
        assert!(
            (info.duration.as_seconds() - 1.0).abs() < 0.15,
            "{name}: {}",
            info.duration.as_seconds()
        );
    }
}

#[test]
fn a_song_is_trimmed_into_the_same_kind_of_file() {
    let ctx = context();
    let song = make_song("trim-song", true);
    let mut viewer = trimmer_on(&ctx, &song);
    viewer.set_trim_start(seconds(0.5));
    viewer.set_trim_end(seconds(2.0));
    viewer.save_trim(&ctx, None);
    let saved = wait_for_save(&ctx, &mut viewer);
    assert_eq!(saved.extension().unwrap(), "mp3");
    let length = bareclip_media::probe(&saved).unwrap().duration.as_seconds();
    assert!((length - 1.5).abs() < 0.06, "{length}");
}

#[test]
fn saving_over_the_original_is_refused_and_a_cancelled_save_leaves_nothing() {
    let ctx = context();
    let folder = folder_of_videos("trim-refused", &["clip.mp4"]);
    let source = folder.join("clip.mp4");
    let before = std::fs::read(&source).unwrap();
    let mut viewer = trimmer_on(&ctx, &source);
    viewer.set_trim_start(seconds(1.0));
    viewer.save_trim(&ctx, Some(source.clone()));
    let problem = viewer
        .trim
        .as_ref()
        .unwrap()
        .problem
        .clone()
        .unwrap_or_default();
    assert!(problem.contains("another name"), "{problem}");
    assert_eq!(std::fs::read(&source).unwrap(), before);

    viewer.save_trim(&ctx, None);
    viewer.cancel_trim_save();
    wait_for(&ctx, &mut viewer, "the cancel", |viewer| {
        !viewer.is_saving_trim()
    });
    let leftovers: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "clip.mp4")
        .collect();
    // A save quick enough to finish before the cancel leaves a whole file, never a part.
    assert!(
        leftovers.is_empty() || leftovers == ["clip (trimmed).mp4"],
        "{leftovers:?}"
    );
}

#[test]
fn the_watching_speed_never_reaches_the_saved_file() {
    let ctx = context();
    let folder = folder_of_videos("trim-speed", &["clip.mp4"]);
    let source = folder.join("clip.mp4");
    let mut viewer = trimmer_on(&ctx, &source);
    viewer.set_trim_start(during_frame(10));
    viewer.set_trim_end(during_frame(70));
    viewer.set_speed(0);
    viewer.save_trim(&ctx, Some(folder.join("slow.mp4")));
    let slow = wait_for_save(&ctx, &mut viewer);
    viewer.set_speed(NORMAL_SPEED);
    viewer.save_trim(&ctx, Some(folder.join("normal.mp4")));
    let normal = wait_for_save(&ctx, &mut viewer);
    assert_eq!(frame_numbers(&slow), frame_numbers(&normal));
    assert_eq!(frame_numbers(&slow), (10..=70).collect::<Vec<u32>>());
}

#[test]
fn copies_are_named_trimmed_then_numbered() {
    let folder = temporary_folder("trim-names");
    let video = folder.join("Clip.mp4");
    assert_eq!(
        trim::copy_name(&video, false),
        folder.join("Clip (trimmed).mp4")
    );
    std::fs::write(folder.join("Clip (trimmed).mp4"), b"").unwrap();
    assert_eq!(
        trim::copy_name(&video, false),
        folder.join("Clip (trimmed 2).mp4")
    );
    assert_eq!(
        trim::copy_name(&folder.join("Old.wmv"), false),
        folder.join("Old (trimmed).mp4"),
        "a kind that can't be kept becomes an MP4"
    );
    assert_eq!(
        trim::copy_name(&folder.join("Song.wma"), true),
        folder.join("Song (trimmed).wma"),
        "sound keeps its kind"
    );
}

#[test]
fn edit_in_bareclip_sends_the_clip_as_trimmed_to_an_open_editor_and_pauses() {
    let ctx = context();
    let prefix = format!("ViewerEditTest{}", std::process::id());
    let editor = crate::handoff::starting_editor(&prefix);
    let mut viewer = trimmer_on(&ctx, video());
    viewer.editor_prefix = prefix;
    viewer.set_trim_start(during_frame(15));
    viewer.set_trim_end(during_frame(74));
    viewer.play();
    click_named(&ctx, &mut viewer, "Edit in BareClip");

    let started = Instant::now();
    let mut arrived = Vec::new();
    while arrived.is_empty() && started.elapsed() < Duration::from_secs(10) {
        arrived = editor.take();
        thread::sleep(Duration::from_millis(5));
    }
    let range = viewer.trim_range();
    assert_eq!(arrived.len(), 1);
    assert_eq!(arrived[0].path, video());
    assert_eq!(arrived[0].range, range, "exactly as trimmed, saved or not");
    assert!(!viewer.is_playing(), "the player pauses and stays");
    assert!(viewer.take_become_editor().is_none());
}

#[test]
fn with_no_editor_open_the_player_asks_to_become_one_with_the_whole_file() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    viewer.editor_prefix = format!("ViewerNoEditor{}", std::process::id());
    viewer.edit_in_bareclip();
    let clip = viewer.take_become_editor().expect("it becomes the editor");
    assert_eq!(clip.path, video());
    assert_eq!(
        clip.range,
        Some(TimeRange {
            start: Ticks(0),
            end: viewer.duration()
        }),
        "outside the trimmer, the whole file"
    );
    assert!(viewer.take_become_editor().is_none(), "only once");
}

/// Saves pictures of the player to look at, in the folder named by `BARECLIP_SCREENS`. Run with
/// `$env:BARECLIP_SCREENS = '<folder>'; ./scripts/cargo.ps1 test -p bareclip-app draw_the_player_screens '--' --ignored`
#[test]
#[ignore = "draws pictures to look at"]
fn draw_the_player_screens() {
    let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
        || std::env::temp_dir().join("bareclip-screens"),
        PathBuf::from,
    );
    screen_camera::SCREEN.set([1280, 720]);
    let ctx = context();
    let mut camera = Camera::new(folder.clone());
    let mut shoot = |viewer: &mut Viewer, events: Vec<egui::Event>, name: &str| {
        for _ in 0..12 {
            frame(&ctx, viewer, vec![]);
        }
        let output = frame(&ctx, viewer, events);
        camera.save(&ctx, output, name);
    };

    let mut empty = Viewer::new(&ctx, false);
    shoot(&mut empty, vec![], "player-01-empty");

    let mut viewer = opened(&ctx, video());
    viewer.seek(seconds(1.0));
    wait_for(&ctx, &mut viewer, "the picture at 1 s", |viewer| {
        viewer.picture.is_some()
    });
    thread::sleep(Duration::from_millis(200));
    shoot(&mut viewer, vec![], "player-02-paused");
    viewer.play();
    shoot(&mut viewer, vec![], "player-03-playing");
    viewer.pause();
    viewer.set_speed(1);
    shoot(&mut viewer, vec![], "player-04-half-speed");
    viewer.full_screen = true;
    viewer.last_pointer_move = Instant::now();
    shoot(&mut viewer, vec![], "player-05-full-screen-bars");
    viewer.last_pointer_move = Instant::now() - BARS_LINGER - Duration::from_millis(10);
    shoot(&mut viewer, vec![], "player-06-full-screen-bare");
    viewer.full_screen = false;
    viewer.settings_open = true;
    shoot(&mut viewer, vec![], "player-07-settings");
    viewer.settings_open = false;

    let broken = temporary_folder("screens").join("broken.mkv");
    std::fs::write(&broken, b"not a video").unwrap();
    let mut viewer = Viewer::new(&ctx, false);
    viewer.open(&ctx, &broken);
    shoot(&mut viewer, vec![], "player-08-unreadable");

    let mut viewer = opened(&ctx, &make_song("screens-cover", true));
    wait_for(&ctx, &mut viewer, "the cover", |viewer| {
        viewer.file.as_ref().is_some_and(|file| file.cover_read)
    });
    shoot(&mut viewer, vec![], "player-09-song-with-cover");
    let mut viewer = opened(&ctx, &make_song("screens-plain", false));
    wait_for(&ctx, &mut viewer, "the cover's read", |viewer| {
        viewer.file.as_ref().is_some_and(|file| file.cover_read)
    });
    shoot(&mut viewer, vec![], "player-10-song-without-cover");

    let mut viewer = opened(&ctx, video());
    viewer.seek(seconds(2.0));
    frame(&ctx, &mut viewer, vec![]);
    let view = viewer.view;
    frame(&ctx, &mut viewer, wheel(view.min + vec2(200.0, 150.0), 2.0));
    wait_for(&ctx, &mut viewer, "the sharp picture", |viewer| {
        viewer.picture.is_some()
    });
    thread::sleep(Duration::from_millis(200));
    shoot(&mut viewer, vec![], "player-11-zoomed");
    let folder_for_picture = temporary_folder("screens-picture");
    let copy = folder_for_picture.join("Clip.mp4");
    std::fs::copy(video(), &copy).unwrap();
    let mut viewer = opened(&ctx, &copy);
    viewer.save_picture(&ctx);
    wait_for(&ctx, &mut viewer, "the picture to save", |viewer| {
        viewer.saved_picture.is_some()
    });
    shoot(&mut viewer, vec![], "player-12-saved-picture");

    screen_camera::SCREEN.set([400, 225]);
    viewer.enter_mini_view(&ctx);
    shoot(
        &mut viewer,
        vec![egui::Event::PointerGone],
        "player-13-mini",
    );
    shoot(
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(200.0, 100.0))],
        "player-14-mini-hovered",
    );
    screen_camera::SCREEN.set([1280, 720]);

    let folder = folder_of_videos("screens-folder", &["a.mp4", "b.mp4", "c.mp4"]);
    let mut viewer = opened(&ctx, &folder.join("b.mp4"));
    wait_for_folder(&ctx, &mut viewer);
    viewer.resumed = Some((seconds(83.0), Instant::now()));
    shoot(
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(640.0, 300.0))],
        "player-15-edge-arrows-and-start-over",
    );

    let folder = folder_of_videos("screens-info", &["Holiday clip.mp4"]);
    let mut viewer = opened(&ctx, &folder.join("Holiday clip.mp4"));
    viewer.toggle_info();
    wait_for_details(&ctx, &mut viewer);
    shoot(&mut viewer, vec![], "player-16-info-video");
    click_named(&ctx, &mut viewer, "File name");
    frame(&ctx, &mut viewer, vec![egui::Event::Text(" 2".to_owned())]);
    shoot(&mut viewer, vec![], "player-17-info-renaming");
    frame(&ctx, &mut viewer, press(Key::Escape));
    viewer.ask_to_delete();
    shoot(&mut viewer, vec![], "player-18-delete-question");
    viewer.confirm_delete = false;
    let mut viewer = opened(&ctx, &make_song("screens-info-song", true));
    viewer.toggle_info();
    wait_for_details(&ctx, &mut viewer);
    shoot(&mut viewer, vec![], "player-19-info-song");

    let folder = folder_of_videos("screens-trim", &["Match.mp4"]);
    let mut viewer = trimmer_on(&ctx, &folder.join("Match.mp4"));
    wait_for(&ctx, &mut viewer, "the strip", |viewer| {
        viewer
            .trim
            .as_ref()
            .is_some_and(|trimmer| trimmer.arriving.is_some())
    });
    for _ in 0..40 {
        frame(&ctx, &mut viewer, vec![]);
        thread::sleep(Duration::from_millis(10));
    }
    viewer.set_trim_start(seconds(0.8));
    viewer.set_trim_end(seconds(3.1));
    viewer.seek(seconds(1.6));
    shoot(&mut viewer, vec![], "player-20-trimmer");
    let saving = viewer.pretend_saving(0.42);
    shoot(&mut viewer, vec![], "player-21-trimmer-saving");
    let _ = saving.send(Ok((
        folder.join("Match (trimmed).mp4"),
        vec!["Copied 60 frames and re-encoded 9.".to_owned()],
    )));
    shoot(&mut viewer, vec![], "player-22-trimmer-saved");
    let mut viewer = trimmer_on(&ctx, &make_song("screens-trim-song", true));
    wait_for(&ctx, &mut viewer, "the sound's shape", |viewer| {
        viewer
            .trim
            .as_ref()
            .is_some_and(|trimmer| trimmer.shape_arriving.is_none())
    });
    viewer.set_trim_start(seconds(0.5));
    viewer.set_trim_end(seconds(2.2));
    shoot(&mut viewer, vec![], "player-23-trimmer-song");

    KEPT_TEXTURES.with_borrow_mut(|kept| *kept = None);
}

/// Times a real launch of the built app from its start to its first picture, on the files in
/// `BARECLIP_LAYER_FILES`, or a generated one. Build first with
/// `./scripts/cargo.ps1 build --release -p bareclip-app`, then run with
/// `./scripts/cargo.ps1 test --release -p bareclip-app first_picture_time '--' --ignored --nocapture`
#[test]
#[ignore = "opens real windows and times them"]
fn first_picture_time() {
    let app = std::env::current_exe()
        .unwrap()
        .parent()
        .and_then(Path::parent)
        .map(|folder| folder.join("bareclip.exe"))
        .filter(|app| app.exists())
        .expect("build bareclip.exe first");
    let files: Vec<PathBuf> = std::env::var("BARECLIP_LAYER_FILES").map_or_else(
        |_| vec![video().to_owned()],
        |list| list.split(';').map(PathBuf::from).collect(),
    );
    let report = temporary_folder("first-picture").join("time.txt");
    for file in files {
        for attempt in 0..3 {
            let _ = std::fs::remove_file(&report);
            let mut child = Command::new(&app)
                .arg(handoff_flag())
                .arg(&file)
                .env(FIRST_PICTURE_VARIABLE, &report)
                .spawn()
                .unwrap();
            let started = Instant::now();
            while !report.exists() && started.elapsed() < Duration::from_secs(20) {
                thread::sleep(Duration::from_millis(20));
            }
            let _ = child.wait();
            let shown = std::fs::read_to_string(&report).unwrap_or_else(|_| "never".to_owned());
            println!("{} attempt {}: {shown} ms", file.display(), attempt + 1);
        }
    }
}

fn handoff_flag() -> &'static str {
    crate::handoff::PLAY_FLAG
}
