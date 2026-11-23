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
    wait_up_to(ctx, viewer, Duration::from_secs(10), what, done);
}

/// Runs frames until something is true, failing after `limit`.
fn wait_up_to(
    ctx: &egui::Context,
    viewer: &mut Viewer,
    limit: Duration,
    what: &str,
    done: impl Fn(&Viewer) -> bool,
) {
    let started = Instant::now();
    while !done(viewer) {
        assert!(started.elapsed() < limit, "waited too long for {what}");
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

/// Somewhere on the picture, away from the bars, the pill and their flyouts.
const ON_THE_PICTURE: egui::Pos2 = egui::pos2(300.0, 300.0);

/// Brings the mouse from the picture to rest on a button long enough for its flyout to open.
fn rest_on(ctx: &egui::Context, viewer: &mut Viewer, name: &str) {
    let rect = named_rect(ctx, viewer, name);
    frame(ctx, viewer, vec![egui::Event::PointerMoved(ON_THE_PICTURE)]);
    frame(ctx, viewer, vec![egui::Event::PointerMoved(rect.center())]);
    viewer.flyout.rest_is_up();
    frame(ctx, viewer, vec![]);
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
    assert_eq!(details[1], "320 \u{d7} 180");
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
fn the_arrows_jump_back_and_forward_each_by_its_own_amount_and_home_and_end_go_to_the_ends() {
    let ctx = context();
    let mut viewer = opened(&ctx, long_video());
    viewer.set_jump(false, 5);
    viewer.set_jump(true, 10);

    frame(&ctx, &mut viewer, press(Key::ArrowRight));
    assert!(near(viewer.position(), seconds(10.0), 0.01));
    frame(&ctx, &mut viewer, press(Key::ArrowRight));
    assert!(near(viewer.position(), seconds(20.0), 0.01));
    frame(&ctx, &mut viewer, press(Key::ArrowLeft));
    assert!(near(viewer.position(), seconds(15.0), 0.01), "back goes 5");
    for _ in 0..4 {
        frame(&ctx, &mut viewer, press(Key::ArrowLeft));
    }
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
fn the_speed_menu_closes_on_a_choice_and_the_speed_is_never_saved() {
    let ctx = context();
    let folder = temporary_folder("speed");
    let mut viewer = Viewer::new(&ctx, false);
    viewer.use_storage(&ctx, folder.clone());
    viewer.open(&ctx, video());

    rest_on(&ctx, &mut viewer, "Watching speed");
    assert_eq!(viewer.flyout.open, Some(bars::Flyout::Speed));
    settle(&ctx, &mut viewer);
    click_named(&ctx, &mut viewer, "1.5x");
    assert_eq!(viewer.speed(), 1.5);
    assert_eq!(viewer.flyout.open, None, "choosing a speed closes the menu");
    settle(&ctx, &mut viewer);
    assert!(
        !named(&ctx, &mut viewer)
            .iter()
            .any(|(label, ..)| label == "1.5x"),
        "and it fades away"
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

/// Says the mouse last moved this long ago, and a little more.
fn rest_for(viewer: &mut Viewer, linger: Duration) {
    viewer.last_pointer_move = Instant::now() - linger - Duration::from_millis(10);
}

#[test]
fn the_pill_goes_when_the_mouse_rests_a_little_later_while_paused_and_moving_brings_it_back() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    rest_for(&mut viewer, PILL_LINGER);
    assert!(viewer.pill_shown(&ctx), "paused, it waits a little longer");
    rest_for(&mut viewer, PILL_LINGER_PAUSED);
    assert!(!viewer.pill_shown(&ctx), "but goes in the end");
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(300.0, 300.0))],
    );
    assert!(viewer.pill_shown(&ctx), "any movement brings it back");

    viewer.play();
    rest_for(&mut viewer, PILL_LINGER);
    assert!(!viewer.pill_shown(&ctx), "playing, it goes sooner");
    settle(&ctx, &mut viewer);
    let shown = named(&ctx, &mut viewer);
    assert!(
        !shown.iter().any(|(label, ..)| label == "Pause"),
        "a hidden pill holds no buttons"
    );
    assert!(
        shown.iter().any(|(label, ..)| label == "Mini view"),
        "the bars stay in a window"
    );
}

#[test]
fn the_pill_stays_while_held_or_under_the_mouse() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    viewer.play();
    viewer.last_pointer_move = Instant::now();
    let on_pill = named_rect(&ctx, &mut viewer, "Loop");
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(on_pill.center())],
    );
    rest_for(&mut viewer, PILL_LINGER);
    assert!(viewer.pill_shown(&ctx), "the mouse resting on it keeps it");
    let middle = viewer.view.center();
    frame(&ctx, &mut viewer, vec![egui::Event::PointerMoved(middle)]);
    rest_for(&mut viewer, PILL_LINGER);
    assert!(
        !viewer.pill_shown(&ctx),
        "resting on the picture lets it go"
    );
    viewer.seek_bar_held = true;
    assert!(viewer.pill_shown(&ctx), "a held seek line keeps it");
    viewer.seek_bar_held = false;
    viewer.settings_open = true;
    assert!(viewer.pill_shown(&ctx), "so does Settings");
}

#[test]
fn in_full_screen_the_bars_come_over_the_picture_only_while_the_mouse_is_over_them() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let screen = screen_camera::screen();
    let shown = |ctx: &egui::Context, viewer: &mut Viewer, name: &str| {
        named(ctx, viewer).iter().any(|(label, ..)| label == name)
    };
    settle(&ctx, &mut viewer);
    assert_eq!(
        viewer.view.top(),
        bars::TITLE_HEIGHT,
        "in a window, under the top bar"
    );
    assert_eq!(viewer.view.bottom(), screen.bottom() - bars::BOTTOM_HEIGHT);

    frame(&ctx, &mut viewer, press(Key::F));
    let middle = screen.center();
    frame(&ctx, &mut viewer, vec![egui::Event::PointerMoved(middle)]);
    settle(&ctx, &mut viewer);
    assert_eq!(viewer.view, screen, "the picture fills the screen");
    assert!(!shown(&ctx, &mut viewer, "Settings"), "the top bar is away");
    assert!(!shown(&ctx, &mut viewer, "Mini view"), "and the bottom one");

    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(300.0, 10.0))],
    );
    settle(&ctx, &mut viewer);
    assert!(shown(&ctx, &mut viewer, "Settings"), "the top bar comes in");
    assert_eq!(viewer.view, screen, "over the picture, not pushing it");
    frame(&ctx, &mut viewer, vec![egui::Event::PointerMoved(middle)]);
    settle(&ctx, &mut viewer);
    assert!(
        !shown(&ctx, &mut viewer, "Settings"),
        "and goes once the mouse leaves it"
    );

    let low = pos2(300.0, screen.bottom() - 10.0);
    frame(&ctx, &mut viewer, vec![egui::Event::PointerMoved(low)]);
    settle(&ctx, &mut viewer);
    let nodes = named(&ctx, &mut viewer);
    assert!(nodes.iter().any(|(label, ..)| label == "Mini view"));
    assert!(
        rect_of(&nodes, "Play").bottom() <= screen.bottom() - bars::BOTTOM_HEIGHT,
        "the pill rises above the bottom bar"
    );

    frame(&ctx, &mut viewer, press(Key::Escape));
    assert!(!viewer.is_full_screen());
}

#[test]
fn the_jumps_start_at_5_s_are_saved_kept_to_the_choices_and_used_by_the_arrows() {
    let ctx = context();
    let folder = temporary_folder("jumps");
    let mut viewer = Viewer::new(&ctx, false);
    viewer.use_storage(&ctx, folder.clone());
    assert_eq!((viewer.jump(false), viewer.jump(true)), (5, 5));

    viewer.set_jump(false, 0);
    assert_eq!(viewer.jump(false), 5);
    viewer.set_jump(true, 90);
    assert_eq!(viewer.jump(true), 30);
    viewer.set_jump(false, 12);
    assert_eq!(
        viewer.jump(false),
        10,
        "between choices it takes the nearest"
    );
    let saved = Settings::load(&folder).viewer;
    assert_eq!((saved.jump_back, saved.jump_forward), (10, 30));

    let mut again = Viewer::new(&ctx, false);
    again.use_storage(&ctx, folder);
    assert_eq!((again.jump(false), again.jump(true)), (10, 30));
    again.open(&ctx, long_video());
    again.pause();
    again.seek(seconds(25.0));
    frame(&ctx, &mut again, press(Key::ArrowLeft));
    assert!(near(again.position(), seconds(15.0), 0.01));
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
fn a_song_shows_its_cover_and_one_without_shows_the_logo_and_name() {
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
    // The logo with the name beside it stands in, centred, in the theme's own colours.
    for (theme, dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
        ctx.set_theme(theme);
        frame(&ctx, &mut viewer, vec![]);
        let output = frame(&ctx, &mut viewer, vec![]);
        let (made_for, texture) = viewer.lockup.clone().expect("the logo and name stand in");
        assert_eq!(made_for, dark, "{theme:?}");
        let drawn = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Mesh(mesh) if mesh.texture_id == texture.id() => {
                    Some(mesh.calc_bounds())
                }
                _ => None,
            })
            .expect("it is drawn");
        assert!(
            drawn.width() > drawn.height() * 3.0,
            "the name sits beside the logo: {drawn:?}"
        );
        assert!(drawn.width() <= LOCKUP_WIDTH + 0.5);
        assert!(
            (drawn.center() - viewer.view.center()).length() < 1.0,
            "centred"
        );
    }
    ctx.set_theme(egui::Theme::Dark);
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
    // On the picture, clear of the bars that float over its edges.
    let corner = view.center() - vec2(200.0, 120.0);

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

    click_named(&ctx, &mut viewer, "Save frame");
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
    click_named(&ctx, &mut viewer, "Save frame");
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
    for name in ["Save frame", "Zoom"] {
        assert!(
            !nodes.iter().any(|(label, _, off)| label == name && !off),
            "{name} is hidden"
        );
    }
}

/// The rectangle of the control with this name.
fn rect_of(nodes: &[(String, Rect, bool)], name: &str) -> Rect {
    nodes
        .iter()
        .find(|(label, ..)| label == name)
        .unwrap_or_else(|| panic!("nothing is named {name}"))
        .1
}

#[test]
fn the_pill_is_56_percent_of_the_pictures_width_and_sits_just_above_the_status_bar() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    settle(&ctx, &mut viewer);
    let check = |viewer: &Viewer| {
        let (pill, view) = (viewer.pill_at, viewer.view);
        assert!(
            (pill.width() - view.width() * pill::PILL_SHARE).abs() < 1.0,
            "{} wide for a picture {} wide",
            pill.width(),
            view.width()
        );
        assert!((pill.center().x - view.center().x).abs() < 1.0, "centred");
        assert_eq!(pill.height(), pill::PILL_HEIGHT);
        assert!((view.bottom() - pill.bottom() - pill::PILL_GAP).abs() < 0.5);
    };
    check(&viewer);

    // Nothing in it moves when the speed changes.
    let before = named(&ctx, &mut viewer);
    viewer.set_speed(0);
    let after = named(&ctx, &mut viewer);
    for name in [
        "Play",
        "Jump back",
        "Jump forward",
        "Volume",
        "Seek",
        "Loop",
    ] {
        assert_eq!(
            rect_of(&before, name),
            rect_of(&after, name),
            "{name} stayed put at 0.25x"
        );
    }

    // With Info open it takes the same share of the narrower picture.
    viewer.toggle_info();
    settle(&ctx, &mut viewer);
    check(&viewer);
}

#[test]
fn the_pill_reads_play_back_forward_volume_seek_loop_save_picture_and_the_bar_the_rest() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let nodes = named(&ctx, &mut viewer);
    let left_of = |name: &str| rect_of(&nodes, name).left();
    let in_order = |names: &[&str]| {
        names
            .windows(2)
            .all(|pair| left_of(pair[0]) < left_of(pair[1]))
    };
    let pill = viewer.pill_at;
    let on_pill = [
        "Play",
        "Jump back",
        "Jump forward",
        "Volume",
        "Seek",
        "Loop",
        "Save frame",
    ];
    assert!(in_order(&on_pill), "the pill runs in order");
    for name in on_pill {
        assert!(
            pill.contains(rect_of(&nodes, name).center()),
            "{name} is on it"
        );
    }
    let bar_top = screen_camera::screen().bottom() - bars::BOTTOM_HEIGHT;
    let in_bar = ["Zoom", "Watching speed", "Mini view", "Full screen"];
    assert!(
        in_order(&in_bar),
        "the status bar runs zoom, speed, mini view, full screen"
    );
    for name in in_bar {
        assert!(
            rect_of(&nodes, name).top() >= bar_top,
            "{name} is in the status bar"
        );
    }
    for gone in ["Go to start", "Go to end", "More options"] {
        assert!(!nodes.iter().any(|(label, ..)| label == gone), "no {gone}");
    }
}

#[test]
fn a_song_has_no_save_picture_so_loop_ends_its_pill() {
    let ctx = context();
    let mut viewer = opened(&ctx, &make_song("pill-end", false));
    let nodes = named(&ctx, &mut viewer);
    assert!(!nodes.iter().any(|(label, ..)| label == "Save frame"));
    let rightmost = nodes
        .iter()
        .filter(|(_, rect, _)| viewer.pill_at.contains(rect.center()))
        .max_by(|a, b| a.1.right().total_cmp(&b.1.right()))
        .map(|(label, ..)| label.clone());
    assert_eq!(rightmost.as_deref(), Some("Loop"));
}

#[test]
fn a_narrow_pill_puts_the_jumps_loop_and_save_picture_into_more() {
    screen_camera::SCREEN.set([640, 400]);
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let nodes = named(&ctx, &mut viewer);
    let left_of = |name: &str| rect_of(&nodes, name).left();
    assert!(left_of("Play") < left_of("Volume"));
    assert!(left_of("Volume") < left_of("Seek"));
    assert!(left_of("Seek") < left_of("More options"));
    for put_away in ["Jump back", "Jump forward", "Loop", "Save frame"] {
        assert!(
            !nodes.iter().any(|(label, ..)| label == put_away),
            "{put_away} is put away"
        );
    }
    click_named(&ctx, &mut viewer, "More options");
    settle(&ctx, &mut viewer);
    let nodes = named(&ctx, &mut viewer);
    for name in ["Jump back", "Jump forward", "Loop", "Save frame"] {
        assert!(
            nodes.iter().any(|(label, ..)| label == name),
            "{name} under More"
        );
    }
    click_named(&ctx, &mut viewer, "Loop");
    assert!(viewer.is_looping(), "Loop works from there");
    assert!(!viewer.flyout.is_open(), "and the menu closes");
}

#[test]
fn notes_stack_without_covering_each_other_and_a_problem_closes() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    viewer.notice =
        Some("Something went wrong in a way that takes a long sentence to say.".to_owned());
    viewer.saved_picture = Some((PathBuf::from("frame.png"), Instant::now()));
    let nodes = named(&ctx, &mut viewer);
    // Both notes can be closed; the problem's is the top one.
    let close = nodes
        .iter()
        .filter(|(label, ..)| label == "Close note")
        .map(|(_, rect, _)| *rect)
        .min_by(|a, b| a.top().total_cmp(&b.top()))
        .expect("the problem's Close");
    let show = rect_of(&nodes, "Show in folder");
    assert!(
        show.top() > close.bottom() + note::NOTE_GAP,
        "the second note is below the first"
    );
    assert!(
        close.top() > viewer.view.top(),
        "notes are over the picture"
    );

    click_at(&ctx, &mut viewer, close.center());
    assert!(viewer.notice.is_none());
    let show_now = named_rect(&ctx, &mut viewer, "Show in folder");
    assert!(
        show_now.top() < show.top(),
        "the note left moves up into the room"
    );
}

#[test]
fn a_saved_trim_says_so_over_the_picture_until_closed() {
    let ctx = context();
    let folder = folder_of_videos("trim-note", &["Match.mp4"]);
    let mut viewer = trimmer_on(&ctx, &folder.join("Match.mp4"));
    let saving = viewer.pretend_saving(0.5);
    let nodes = named(&ctx, &mut viewer);
    assert!(
        nodes.iter().any(|(label, ..)| label == "Cancel saving"),
        "its progress shows under the strip"
    );
    assert!(
        nodes
            .iter()
            .any(|(label, _, off)| label == trim::SAVE_AS && *off),
        "the save buttons wait while one is under way"
    );
    let _ = saving.send(Ok((
        folder.join("Match (trimmed).mp4"),
        vec!["Copied it all.".to_owned()],
    )));
    frame(&ctx, &mut viewer, vec![]);
    let nodes = named(&ctx, &mut viewer);
    let note = rect_of(&nodes, "Play it");
    assert!(
        note.bottom() < viewer.view.center().y,
        "the note is at the top"
    );
    assert!(nodes.iter().any(|(label, ..)| label == trim::SAVE_AS));

    click_named(&ctx, &mut viewer, "Close note");
    assert!(viewer.trim.as_ref().unwrap().saved.is_none());
    assert!(
        viewer.is_trimming(),
        "closing the note leaves the trimmer open"
    );
}

#[test]
fn the_volume_button_mutes_on_a_click_and_opens_its_slider_while_the_mouse_rests_on_it() {
    assert_eq!(bars::volume_icon(0), icons::VOLUME_X);
    assert_eq!(bars::volume_icon(20), icons::VOLUME);
    assert_eq!(bars::volume_icon(50), icons::VOLUME_1);
    assert_eq!(bars::volume_icon(100), icons::VOLUME_2);

    let ctx = context();
    let mut viewer = opened(&ctx, video());
    viewer.set_volume(60);
    click_named(&ctx, &mut viewer, "Volume");
    assert!(viewer.is_muted(), "a click mutes");
    assert_eq!(viewer.flyout.open, None, "and opens nothing");
    click_named(&ctx, &mut viewer, "Volume");
    assert!(!viewer.is_muted());
    assert_eq!(
        viewer.volume(),
        60,
        "a second click brings it back as it was"
    );

    let closed = named(&ctx, &mut viewer);
    assert!(!closed.iter().any(|(label, ..)| label == "Volume level"));
    rest_on(&ctx, &mut viewer, "Volume");
    assert_eq!(viewer.flyout.open, Some(bars::Flyout::Volume));
    settle(&ctx, &mut viewer);
    let open = named(&ctx, &mut viewer);
    let slider = rect_of(&open, "Volume level");
    assert!(
        slider.bottom() < rect_of(&open, "Volume").top(),
        "the flyout opens above the bar"
    );
    click_at(
        &ctx,
        &mut viewer,
        slider.left_center() + vec2(slider.width() * 0.25, 0.0),
    );
    assert!(
        (15..=35).contains(&viewer.volume()),
        "a click a quarter along sets about a quarter: {}",
        viewer.volume()
    );
    click_named(&ctx, &mut viewer, "Mute");
    assert!(viewer.is_muted());
    click_named(&ctx, &mut viewer, "Volume");
    assert!(!viewer.is_muted(), "the button unmutes with it open");
    assert_eq!(
        viewer.flyout.open,
        Some(bars::Flyout::Volume),
        "and leaves it open"
    );

    // Escape closes the flyout, and only the flyout, even in full screen.
    viewer.full_screen = true;
    frame(&ctx, &mut viewer, press(Key::Escape));
    settle(&ctx, &mut viewer);
    assert!(
        !named(&ctx, &mut viewer)
            .iter()
            .any(|(label, ..)| label == "Volume level")
    );
    assert!(
        viewer.is_full_screen(),
        "the first Escape only closed the flyout"
    );
    viewer.full_screen = false;
    settle(&ctx, &mut viewer);

    // The wheel over the button turns it up.
    let before = viewer.volume();
    let button = named_rect(&ctx, &mut viewer, "Volume");
    frame(&ctx, &mut viewer, wheel(button.center(), 2.0));
    assert_eq!(viewer.volume(), (before + 2 * VOLUME_STEP).min(100));
}

#[test]
fn the_zoom_flyout_has_a_slider_over_minus_percentage_plus_and_reset() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    frame(&ctx, &mut viewer, vec![]);
    let fit = viewer.zoom_percent(&ctx);
    rest_on(&ctx, &mut viewer, "Zoom");
    settle(&ctx, &mut viewer);
    let nodes = named(&ctx, &mut viewer);
    let (slider, minus, plus, reset) = (
        rect_of(&nodes, "Zoom level"),
        rect_of(&nodes, "Zoom out"),
        rect_of(&nodes, "Zoom in"),
        rect_of(&nodes, "Reset zoom"),
    );
    assert!(slider.bottom() <= minus.top(), "the slider is on top");
    assert!(minus.right() < plus.left() && plus.right() < reset.left());
    assert!(
        (slider.left() - minus.left()).abs() <= 1.0
            && (slider.right() - reset.right()).abs() <= 1.0,
        "the slider is as long as the row under it: {slider:?}, {minus:?} to {reset:?}"
    );
    assert!(reset.left() - plus.right() >= 10.0, "reset is set apart");

    // − and + go by tens, past fit both ways, stopping at fit on the way through.
    click_named(&ctx, &mut viewer, "Zoom in");
    let once = viewer.zoom_percent(&ctx);
    assert!(
        (once - ((fit / 10.0).floor() + 1.0) * 10.0).abs() < 0.5,
        "{fit} went to {once}"
    );
    click_named(&ctx, &mut viewer, "Zoom out");
    assert!(viewer.zoom.is_fit(), "back down, it stops at fit");
    click_named(&ctx, &mut viewer, "Zoom out");
    let below = viewer.zoom_percent(&ctx);
    assert!(below < fit, "and goes on below it: {below} under {fit}");

    // The slider's middle is fit, its left end a quarter of fit.
    let slider = named_rect(&ctx, &mut viewer, "Zoom level");
    click_at(&ctx, &mut viewer, slider.center());
    assert!(viewer.zoom.is_fit(), "the middle is fit");
    click_at(&ctx, &mut viewer, slider.left_center());
    assert!(
        (viewer.zoom_percent(&ctx) - fit * zoom::LEAST_OF_FIT).abs() < 0.5,
        "the left end is a quarter of fit"
    );
    let picture = viewer
        .zoom
        .picture_rect(viewer.view, viewer.video_size().unwrap());
    assert!(
        picture.width() < viewer.view.width(),
        "the picture is smaller"
    );
    click_at(&ctx, &mut viewer, slider.right_center());
    assert!(
        viewer.zoom_percent(&ctx) > 700.0,
        "the right end is the closest zoom"
    );

    click_named(&ctx, &mut viewer, "Reset zoom");
    assert!(viewer.zoom.is_fit(), "reset goes back to fit");
}

#[test]
fn zoom_steps_land_on_tens_and_100_means_real_pixels() {
    let ctx = context();
    // A video bigger than the window, so fit is below 100%.
    let big = temporary_folder("zoom-tens").join("big.mp4");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .args(["-i", "testsrc2=size=2560x1440:rate=30:duration=1"])
        .args(["-pix_fmt", "yuv420p"])
        .arg(&big)
        .status()
        .unwrap();
    assert!(status.success());
    let mut viewer = opened(&ctx, &big);
    frame(&ctx, &mut viewer, vec![]);
    let fit = viewer.zoom_percent(&ctx);
    assert!(fit < 100.0, "fit is {fit}");
    viewer.zoom_to(&ctx, 100.0);
    frame(&ctx, &mut viewer, vec![]);
    assert!((viewer.zoom_percent(&ctx) - 100.0).abs() < 0.5);
    viewer.zoom_step(&ctx, true);
    frame(&ctx, &mut viewer, vec![]);
    assert!((viewer.zoom_percent(&ctx) - 110.0).abs() < 0.5);
    viewer.zoom_step(&ctx, false);
    viewer.zoom_step(&ctx, false);
    frame(&ctx, &mut viewer, vec![]);
    assert!((viewer.zoom_percent(&ctx) - 90.0).abs() < 0.5);
}

#[test]
fn the_jump_buttons_each_move_by_their_own_amount_and_show_it() {
    let ctx = context();
    let mut viewer = opened(&ctx, long_video());
    viewer.set_jump(false, 10);
    viewer.set_jump(true, 15);
    let output = frame(&ctx, &mut viewer, vec![]);
    let (back, forward) = (
        named_rect(&ctx, &mut viewer, "Jump back"),
        named_rect(&ctx, &mut viewer, "Jump forward"),
    );
    assert!(back.contains(text_rect(&output, "10").center()));
    assert!(forward.contains(text_rect(&output, "15").center()));
    click_named(&ctx, &mut viewer, "Jump forward");
    assert!(near(viewer.position(), seconds(15.0), 0.05));
    click_named(&ctx, &mut viewer, "Jump back");
    assert!(near(viewer.position(), seconds(5.0), 0.05));
}

#[test]
fn the_jump_arrows_arch_lower_than_they_are_wide() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let button = named_rect(&ctx, &mut viewer, "Jump back");
    let output = frame(&ctx, &mut viewer, vec![]);
    // The arrow's lines, which are the only lines in the button above its number.
    let mut arrow = Rect::NOTHING;
    for clipped in &output.shapes {
        if let egui::epaint::Shape::Path(path) = &clipped.shape {
            for point in &path.points {
                if button.contains(*point) && point.y < button.center().y + 2.0 {
                    arrow.extend_with(*point);
                }
            }
        }
    }
    assert!(arrow.is_positive(), "the arrow is drawn");
    assert!(
        arrow.height() < arrow.width() * 0.6,
        "a low arch: {} tall and {} wide",
        arrow.height(),
        arrow.width()
    );
}

#[test]
fn while_trimming_save_as_and_cancel_are_at_the_top_left_and_info_beside_trim() {
    screen_camera::SCREEN.set([1280, 720]);
    let ctx = context();
    let mut viewer = trimmer_on(&ctx, video());
    let output = frame(&ctx, &mut viewer, vec![]);
    let nodes = named(&ctx, &mut viewer);
    let (save, cancel) = (rect_of(&nodes, trim::SAVE_AS), rect_of(&nodes, "Cancel"));
    let screen = screen_camera::screen();
    assert_eq!(save.size(), cancel.size(), "the same size");
    assert!(save.right() < cancel.left(), "Save as…, then Cancel");
    assert_eq!(save.center().y, cancel.center().y, "side by side");
    for rect in [save, cancel] {
        assert!(rect.bottom() <= bars::TITLE_HEIGHT, "in the top bar");
    }
    assert!(save.left() < 20.0, "at the left: {save:?}");
    assert!(
        !output.shapes.iter().any(|clipped| matches!(
            &clipped.shape,
            egui::epaint::Shape::Text(shape) if shape.galley.text().contains("viewer-")
        )),
        "in place of the file's name"
    );

    // "Trim" and Info in the middle, centred together.
    let word = text_rect(&output, "Trim");
    let info = rect_of(&nodes, "Info");
    assert!(word.right() < info.left(), "Info is right of Trim");
    assert!(info.bottom() <= bars::TITLE_HEIGHT);
    let group_middle = (word.left() - 24.0 + info.right()) / 2.0;
    assert!(
        (group_middle - screen.center().x).abs() <= 2.0,
        "centred together: {group_middle}"
    );

    // The pill keeps clear of the window's bottom edge.
    let play = rect_of(&nodes, "Play");
    let pill_bottom = play.center().y + pill::PILL_HEIGHT / 2.0;
    assert!(
        (screen.bottom() - pill_bottom - trim::PILL_BOTTOM_ROOM).abs() < 1.0,
        "{} below the pill",
        screen.bottom() - pill_bottom
    );
    assert!(
        (viewer.view.bottom() - (screen.bottom() - trim::TRIM_BAR_HEIGHT)).abs() < 1.0,
        "the picture comes down to just over the pill"
    );
    for gone in ["Trim", "Delete", "Edit in BareClip", "Settings", "Loop"] {
        assert!(
            !nodes.iter().any(|(label, ..)| label == gone),
            "{gone} is put away while trimming"
        );
    }
    // The pill along the bottom: play, volume, the line with its handles and More, in order.
    let left_of = |name: &str| rect_of(&nodes, name).left();
    assert!(left_of("Play") < left_of("Volume"));
    assert!(left_of("Volume") < left_of("Trim start"));
    assert!(left_of("Trim start") < left_of("Trim end"));
    assert!(left_of("Trim end") < left_of("More options"));
    let pill_middle = (left_of("Play") + rect_of(&nodes, "More options").right()) / 2.0;
    assert!(
        (pill_middle - screen_camera::screen().center().x).abs() <= 12.0,
        "the pill sits in the middle"
    );
    // From its first button to its last, inside the room the pill keeps at its ends.
    let pill_width = rect_of(&nodes, "More options").right() - left_of("Play") + 16.0;
    assert!(
        (pill_width - viewer.view.width() * pill::PILL_SHARE).abs() <= 8.0,
        "as wide as the player's pill: {pill_width}"
    );

    // The trimmer's tools are under More, and Save… and Edit in BareClip are gone.
    click_named(&ctx, &mut viewer, "More options");
    settle(&ctx, &mut viewer);
    let nodes = named(&ctx, &mut viewer);
    for name in [
        "Start at playhead",
        "End at playhead",
        "Reset to whole clip",
        "Loop the kept part",
        "Save without sound",
    ] {
        assert!(
            nodes.iter().any(|(label, ..)| label == name),
            "{name} under More"
        );
    }
    for gone in ["Save…", "Edit in BareClip"] {
        assert!(!nodes.iter().any(|(label, ..)| label == gone), "no {gone}");
    }
    frame(&ctx, &mut viewer, press(Key::Escape));
    settle(&ctx, &mut viewer);
    assert!(viewer.is_trimming(), "Escape closed only the menu");

    click_named(&ctx, &mut viewer, "Cancel");
    assert!(!viewer.is_trimming());
    let nodes = named(&ctx, &mut viewer);
    assert!(!nodes.iter().any(|(label, ..)| label == trim::SAVE_AS));
}

#[test]
fn the_name_is_centred_on_the_window_unless_the_buttons_crowd_it() {
    let bar = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 48.0));
    let left = Rect::from_min_size(pos2(8.0, 8.0), vec2(120.0, 32.0));
    let right = Rect::from_min_size(pos2(1000.0, 8.0), vec2(272.0, 32.0));
    let room = bars::name_room(bar, left, right);
    assert!((room.center().x - 640.0).abs() < 0.01, "centred: {room:?}");
    assert!(room.right() <= right.left(), "clear of the buttons");

    let crowded = Rect::from_min_size(pos2(8.0, 8.0), vec2(560.0, 32.0));
    let room = bars::name_room(bar, crowded, right);
    assert!(
        room.left() >= crowded.right(),
        "it moves over into the room left"
    );
}

#[test]
fn settings_has_general_and_keys_tabs_and_the_version_like_the_editors() {
    screen_camera::SCREEN.set([1280, 720]);
    let ctx = context();
    let folder = temporary_folder("settings-tabs");
    let mut viewer = Viewer::new(&ctx, false);
    viewer.use_storage(&ctx, folder.clone());
    viewer.open(&ctx, video());
    viewer.is_default_player = || false;
    viewer.settings_open = true;
    settle(&ctx, &mut viewer);
    let output = frame(&ctx, &mut viewer, vec![]);
    assert!(text_rect(&output, &text::version_line()).is_positive());
    let nodes = named(&ctx, &mut viewer);
    for name in [
        "General settings",
        "Keys settings",
        "Jump back: 5 s",
        "Jump forward: 5 s",
        "Make default",
        "Done",
    ] {
        assert!(
            nodes.iter().any(|(label, ..)| label == name),
            "{name} is on the General tab"
        );
    }
    let done = rect_of(&nodes, "Done");

    click_named(&ctx, &mut viewer, "Jump forward: 5 s");
    settle(&ctx, &mut viewer);
    let mut choices: Vec<(String, Rect)> = named(&ctx, &mut viewer)
        .into_iter()
        .filter(|(label, ..)| label.ends_with(" s") && !label.contains(':'))
        .map(|(label, rect, ..)| (label, rect))
        .collect();
    choices.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
    let choices: Vec<String> = choices.into_iter().map(|(label, _)| label).collect();
    assert_eq!(choices, ["5 s", "10 s", "15 s", "20 s", "30 s"]);
    click_named(&ctx, &mut viewer, "30 s");
    settle(&ctx, &mut viewer);
    assert_eq!(viewer.jump(true), 30);
    assert_eq!(viewer.jump(false), 5, "the other jump is its own");
    assert_eq!(
        Settings::load(&folder).viewer.jump_forward,
        30,
        "and it's saved"
    );

    click_named(&ctx, &mut viewer, "Keys settings");
    settle(&ctx, &mut viewer);
    let output = frame(&ctx, &mut viewer, vec![]);
    for (_, what) in KEY_LIST {
        assert!(text_rect(&output, what).is_positive(), "{what} is listed");
    }
    let nodes = named(&ctx, &mut viewer);
    assert!(!nodes.iter().any(|(label, ..)| label == "Make default"));
    assert_eq!(rect_of(&nodes, "Done"), done, "it keeps its size");
}

#[test]
fn settings_fit_a_720_high_window_and_scroll_in_a_shorter_one() {
    screen_camera::SCREEN.set([1280, 720]);
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    viewer.is_default_player = || false;
    viewer.settings_open = true;
    for _ in 0..12 {
        frame(&ctx, &mut viewer, vec![]);
    }
    let nodes = named(&ctx, &mut viewer);
    let done = rect_of(&nodes, "Done");
    let height = screen_camera::SCREEN.get()[1] as f32;
    assert!(done.bottom() <= height, "Done is on screen at {done:?}");
    assert!(
        nodes.iter().any(|(label, ..)| label == "Make default"),
        "every row shows"
    );

    screen_camera::SCREEN.set([960, 400]);
    for _ in 0..12 {
        frame(&ctx, &mut viewer, vec![]);
    }
    let done = named_rect(&ctx, &mut viewer, "Done");
    screen_camera::SCREEN.set([1280, 720]);
    assert!(
        done.bottom() <= 400.0,
        "Done is still on screen at {done:?}"
    );
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

/// A 30 second video, small and slow so it is quick to make, longer than any jump.
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
fn every_file_starts_at_its_beginning() {
    screen_camera::SCREEN.set([1280, 720]);
    let ctx = context();
    let storage = temporary_folder("starts-at-the-beginning");
    let folder = folder_of_videos("starts-at-the-beginning", &["a.mp4", "b.mp4", "c.mp4"]);
    let mut viewer = Viewer::new(&ctx, false);
    viewer.use_storage(&ctx, storage.clone());
    viewer.open(&ctx, &folder.join("a.mp4"));
    wait_for(&ctx, &mut viewer, "the folder", |viewer| {
        viewer.next_file().is_some()
    });
    let at_the_start = |viewer: &Viewer| viewer.position() < seconds(0.5);

    // Watched partway, the next file by its arrow starts at its beginning.
    viewer.seek(seconds(2.0));
    settle(&ctx, &mut viewer);
    let middle = viewer.view.center();
    frame(&ctx, &mut viewer, vec![egui::Event::PointerMoved(middle)]);
    click_named(&ctx, &mut viewer, "Next file");
    assert_eq!(viewer.current_path(), Some(folder.join("b.mp4").as_path()));
    assert!(at_the_start(&viewer), "at {:?}", viewer.position());

    // And by Page Down, and going back to one watched partway.
    viewer.seek(seconds(2.0));
    settle(&ctx, &mut viewer);
    frame(&ctx, &mut viewer, press(Key::PageDown));
    assert!(at_the_start(&viewer), "at {:?}", viewer.position());
    viewer.seek(seconds(2.0));
    settle(&ctx, &mut viewer);
    frame(&ctx, &mut viewer, press(Key::PageUp));
    assert_eq!(viewer.current_path(), Some(folder.join("b.mp4").as_path()));
    assert!(at_the_start(&viewer), "at {:?}", viewer.position());

    // A file sent from Explorer too, and the same file after the app is closed and opened.
    viewer.seek(seconds(2.0));
    settle(&ctx, &mut viewer);
    viewer.open(&ctx, &folder.join("c.mp4"));
    assert!(at_the_start(&viewer), "at {:?}", viewer.position());
    viewer.seek(seconds(2.0));
    viewer.closing(None);
    let mut again = Viewer::new(&ctx, false);
    again.use_storage(&ctx, storage);
    again.open(&ctx, &folder.join("c.mp4"));
    assert!(at_the_start(&again), "at {:?}", again.position());
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

/// Waits for the info panel's details, and for the card to finish sliding in.
fn wait_for_details(ctx: &egui::Context, viewer: &mut Viewer) {
    wait_for(ctx, viewer, "the details", |viewer| {
        viewer.info.as_ref().is_some_and(|info| info.has_details())
    });
    settle(ctx, viewer);
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
    type_text(&ctx, &mut viewer, info::MADE_LABEL, "2019-05-06 07:08:09");
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
    assert!(!nodes.iter().any(|(label, ..)| label == info::MADE_LABEL));
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
    // A save is a real encode, which takes far longer while the whole suite runs at once, as
    // the export tests allow for.
    wait_up_to(ctx, viewer, Duration::from_secs(60), "the save", |viewer| {
        !viewer.is_saving_trim()
    });
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
fn a_trim_is_saved_frame_exact_where_the_save_window_says() {
    let ctx = context();
    let folder = folder_of_videos("trim-exact", &["numbers.mp4"]);
    let source = folder.join("numbers.mp4");
    let mut viewer = trimmer_on(&ctx, &source);
    // As if the Save window were left on the name it suggests.
    viewer.pick_trim_target = |suggested| Some(suggested.to_owned());
    viewer.set_trim_start(during_frame(17));
    viewer.set_trim_end(during_frame(82));
    click_named(&ctx, &mut viewer, trim::SAVE_AS);
    let saved = wait_for_save(&ctx, &mut viewer);
    assert_eq!(saved, folder.join("numbers (trimmed).mp4"));
    assert_eq!(frame_numbers(&saved), (17..=82).collect::<Vec<u32>>());
    assert!(viewer.is_trimming(), "the trimmer stays open");
    let summary = &viewer.trim.as_ref().unwrap().saved.as_ref().unwrap().1;
    assert!(!summary.is_empty(), "it says how it was saved");

    // The next suggestion doesn't overwrite the first.
    click_named(&ctx, &mut viewer, trim::SAVE_AS);
    assert_eq!(
        wait_for_save(&ctx, &mut viewer),
        folder.join("numbers (trimmed 2).mp4")
    );
    click_named(&ctx, &mut viewer, "Play it");
    assert_eq!(
        viewer.current_path(),
        Some(folder.join("numbers (trimmed 2).mp4").as_path()),
        "Play opens the new file here"
    );
}

/// What the made-up Save window was offered, and where it says to save.
static SAVE_WINDOW: std::sync::Mutex<(Option<PathBuf>, Option<PathBuf>)> =
    std::sync::Mutex::new((None, None));

#[test]
fn save_as_asks_where_starting_beside_the_original_and_cancelling_saves_nothing() {
    let ctx = context();
    let folder = folder_of_videos("trim-save-as", &["clip.mp4"]);
    let elsewhere = folder.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let mut viewer = trimmer_on(&ctx, &folder.join("clip.mp4"));
    viewer.pick_trim_target = |suggested| {
        let mut window = SAVE_WINDOW.lock().unwrap();
        window.0 = Some(suggested.to_owned());
        window.1.clone()
    };
    viewer.set_trim_end(seconds(1.0));

    // Closing the Save window leaves everything as it was.
    click_named(&ctx, &mut viewer, trim::SAVE_AS);
    assert_eq!(
        SAVE_WINDOW.lock().unwrap().0,
        Some(folder.join("clip (trimmed).mp4")),
        "it starts beside the original with the copy's name"
    );
    assert!(!viewer.is_saving_trim());
    assert!(!folder.join("clip (trimmed).mp4").exists());

    // A name typed without its extension gets the kind of file it is.
    SAVE_WINDOW.lock().unwrap().1 = Some(elsewhere.join("Best bit"));
    click_named(&ctx, &mut viewer, trim::SAVE_AS);
    assert_eq!(
        wait_for_save(&ctx, &mut viewer),
        elsewhere.join("Best bit.mp4")
    );
    assert!(elsewhere.join("Best bit.mp4").exists());
    assert!(
        !folder.join("clip (trimmed).mp4").exists(),
        "nothing beside it"
    );
}

#[test]
fn a_picked_name_ends_in_the_kind_the_trim_is() {
    let suggested = Path::new(r"C:\v\clip (trimmed).mp4");
    for (chosen, saved) in [
        (r"D:\out\best.mp4", r"D:\out\best.mp4"),
        (r"D:\out\best.MP4", r"D:\out\best.MP4"),
        (r"D:\out\best", r"D:\out\best.mp4"),
        (r"D:\out\best.mkv", r"D:\out\best.mkv.mp4"),
        (r"D:\out\v1.2 final", r"D:\out\v1.2 final.mp4"),
    ] {
        assert_eq!(
            trim::with_extension_of(PathBuf::from(chosen), suggested),
            PathBuf::from(saved),
            "{chosen}"
        );
    }
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
            "viewer::tests::a_trim_is_saved_frame_exact_where_the_save_window_says",
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
        "the whole file"
    );
    assert!(viewer.take_become_editor().is_none(), "only once");
}

/// Saves pictures of the player to look at, in the folder named by `BARECLIP_SCREENS`. Run with
/// `$env:BARECLIP_SCREENS = '<folder>'; ./scripts/cargo.ps1 test -p bareclip-app draw_the_player_screens '--' --ignored`
#[test]
#[ignore = "draws pictures to look at"]
fn draw_the_player_screens() {
    draw_player_screens_in(egui::Theme::Dark, "");
    draw_player_screens_in(egui::Theme::Light, "-light");
}

/// Every picture of the player in one theme, with `suffix` after each name.
fn draw_player_screens_in(theme: egui::Theme, suffix: &str) {
    let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
        || std::env::temp_dir().join("bareclip-screens"),
        PathBuf::from,
    );
    screen_camera::SCREEN.set([1280, 720]);
    let ctx = context();
    ctx.set_theme(theme);
    let mut camera = Camera::new(folder.clone());
    // How many frames run before a picture, so menus have measured themselves; none to catch
    // something partway through moving.
    let settling = std::cell::Cell::new(12);
    let mut shoot = |viewer: &mut Viewer, events: Vec<egui::Event>, name: &str| {
        for _ in 0..settling.get() {
            frame(&ctx, viewer, vec![]);
        }
        let output = frame(&ctx, viewer, events);
        camera.save(&ctx, output, &format!("{name}{suffix}"));
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
    shoot(
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(300.0, 10.0))],
        "player-05-full-screen-top-bar",
    );
    shoot(
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(300.0, 712.0))],
        "player-05b-full-screen-bottom-bar",
    );
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(640.0, 300.0))],
    );
    viewer.last_pointer_move = Instant::now() - PILL_LINGER_PAUSED - Duration::from_millis(10);
    shoot(&mut viewer, vec![], "player-06-full-screen-bare");
    viewer.toggle_info();
    shoot(&mut viewer, vec![], "player-06b-full-screen-info");
    viewer.toggle_info();
    settle(&ctx, &mut viewer);
    viewer.full_screen = false;
    viewer.is_default_player = || false;
    viewer.settings_open = true;
    shoot(&mut viewer, vec![], "player-07-settings");
    viewer.settings_open = false;
    frame(&ctx, &mut viewer, vec![]);
    viewer.is_default_player = || true;
    viewer.settings_open = true;
    shoot(&mut viewer, vec![], "player-07c-settings-default");
    viewer.is_default_player = || false;
    viewer.settings_tab = SettingsTab::Keys;
    shoot(&mut viewer, vec![], "player-07b-settings-keys");
    viewer.settings_tab = SettingsTab::General;
    viewer.settings_open = false;
    frame(&ctx, &mut viewer, vec![]);
    viewer.set_speed(NORMAL_SPEED);
    rest_on(&ctx, &mut viewer, "Volume");
    shoot(&mut viewer, vec![], "player-26-volume-flyout");
    frame(&ctx, &mut viewer, press(Key::Escape));
    rest_on(&ctx, &mut viewer, "Zoom");
    shoot(&mut viewer, vec![], "player-27-zoom-flyout");
    frame(&ctx, &mut viewer, press(Key::Escape));
    viewer.zoom_to(&ctx, 40.0);
    shoot(&mut viewer, vec![], "player-32-zoomed-out");
    viewer.fit();
    rest_on(&ctx, &mut viewer, "Watching speed");
    shoot(&mut viewer, vec![], "player-31-speed-flyout");
    frame(&ctx, &mut viewer, press(Key::Escape));
    // Partway: a flyout growing out of its button, and the bars sliding away.
    for _ in 0..12 {
        frame(&ctx, &mut viewer, vec![]);
    }
    rest_on(&ctx, &mut viewer, "Volume");
    settling.set(0);
    shoot(&mut viewer, vec![], "player-33-flyout-growing");
    settling.set(12);
    frame(&ctx, &mut viewer, press(Key::Escape));
    for _ in 0..12 {
        frame(&ctx, &mut viewer, vec![]);
    }
    viewer.play();
    viewer.last_pointer_move = Instant::now() - PILL_LINGER - Duration::from_millis(10);
    frame(&ctx, &mut viewer, vec![]);
    settling.set(0);
    shoot(&mut viewer, vec![], "player-34-pill-going");
    settling.set(12);
    viewer.pause();
    viewer.last_pointer_move = Instant::now();
    let seek = named_rect(&ctx, &mut viewer, "Seek");
    shoot(
        &mut viewer,
        vec![egui::Event::PointerMoved(
            seek.left_center() + vec2(seek.width() * 0.6, 0.0),
        )],
        "player-28-seek-hover",
    );
    viewer.play();
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(640.0, 300.0))],
    );
    viewer.last_pointer_move = Instant::now() - PILL_LINGER - Duration::from_millis(10);
    shoot(&mut viewer, vec![], "player-29-playing-pill-hidden");
    viewer.last_pointer_move = Instant::now();
    shoot(&mut viewer, vec![], "player-30-playing-pill-over-picture");
    viewer.pause();

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
    shoot(
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(640.0, 300.0))],
        "player-15-edge-arrows",
    );

    let folder = folder_of_videos("screens-info", &["Holiday clip.mp4"]);
    let mut viewer = opened(&ctx, &folder.join("Holiday clip.mp4"));
    viewer.toggle_info();
    wait_for_details(&ctx, &mut viewer);
    shoot(&mut viewer, vec![], "player-16-info-video");
    let created = named_rect(&ctx, &mut viewer, "Created");
    shoot(
        &mut viewer,
        vec![egui::Event::PointerMoved(created.center())],
        "player-16b-info-row-hovered",
    );
    click_named(&ctx, &mut viewer, "File name");
    frame(&ctx, &mut viewer, vec![egui::Event::Text(" 2".to_owned())]);
    shoot(&mut viewer, vec![], "player-17-info-renaming");
    frame(&ctx, &mut viewer, press(Key::Escape));
    frame(&ctx, &mut viewer, vec![]);
    viewer.toggle_info();
    for _ in 0..6 {
        frame(&ctx, &mut viewer, vec![]);
    }
    settling.set(0);
    shoot(&mut viewer, vec![], "player-17b-info-sliding-away");
    settling.set(12);
    viewer.toggle_info();
    wait_for_details(&ctx, &mut viewer);
    viewer.ask_to_delete();
    shoot(&mut viewer, vec![], "player-18-delete-question");
    viewer.confirm_delete = false;
    let mut viewer = opened(&ctx, &make_song("screens-info-song", true));
    viewer.toggle_info();
    wait_for_details(&ctx, &mut viewer);
    shoot(&mut viewer, vec![], "player-19-info-song");

    let folder = folder_of_videos("screens-trim", &["Match.mp4"]);
    let mut viewer = trimmer_on(&ctx, &folder.join("Match.mp4"));
    viewer.set_trim_start(seconds(0.8));
    viewer.set_trim_end(seconds(3.1));
    viewer.seek(seconds(1.6));
    shoot(&mut viewer, vec![], "player-20-trimmer");
    click_named(&ctx, &mut viewer, "More options");
    shoot(&mut viewer, vec![], "player-20b-trimmer-more");
    frame(&ctx, &mut viewer, press(Key::Escape));
    let saving = viewer.pretend_saving(0.42);
    shoot(&mut viewer, vec![], "player-21-trimmer-saving");
    let _ = saving.send(Ok((
        folder.join("Match (trimmed).mp4"),
        vec!["Copied 60 frames and re-encoded 9.".to_owned()],
    )));
    shoot(&mut viewer, vec![], "player-22-trimmer-saved");
    let mut viewer = trimmer_on(&ctx, &make_song("screens-trim-song", true));
    viewer.set_trim_start(seconds(0.5));
    viewer.set_trim_end(seconds(2.2));
    shoot(&mut viewer, vec![], "player-23-trimmer-song");
    screen_camera::SCREEN.set([640, 340]);
    shoot(&mut viewer, vec![], "player-23b-trimmer-smallest-window");
    screen_camera::SCREEN.set([1280, 720]);

    let mut viewer = opened(&ctx, video());
    viewer.notice = Some(
        "Your settings could not be saved. The folder they live in can't be written to, so \
         nothing changed here will be kept after the player closes."
            .to_owned(),
    );
    viewer.saved_picture = Some((PathBuf::from("frame.png"), Instant::now()));
    shoot(&mut viewer, vec![], "player-24-notes-stacked");
    viewer.notice = None;
    viewer.saved_picture = None;
    screen_camera::SCREEN.set([640, 400]);
    shoot(
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(320.0, 200.0))],
        "player-35-narrow-pill",
    );
    viewer.toggle_info();
    shoot(&mut viewer, vec![], "player-35b-narrow-info");
    viewer.toggle_info();
    settle(&ctx, &mut viewer);
    screen_camera::SCREEN.set([960, 540]);
    viewer.settings_open = true;
    shoot(&mut viewer, vec![], "player-25-settings-short-window");
    screen_camera::SCREEN.set([1280, 720]);

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

/// Runs frames until whatever is fading in or out has finished.
fn settle(ctx: &egui::Context, viewer: &mut Viewer) {
    for _ in 0..30 {
        frame(ctx, viewer, vec![]);
    }
}

/// One frame with the window said to be maximised or full screen.
fn frame_in_window(
    ctx: &egui::Context,
    viewer: &mut Viewer,
    maximized: bool,
    fullscreen: bool,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let mut input = egui::RawInput {
        screen_rect: Some(screen_camera::screen()),
        events,
        ..Default::default()
    };
    input.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            maximized: Some(maximized),
            fullscreen: Some(fullscreen),
            ..Default::default()
        },
    );
    let mut output = ctx.run_ui(input, |ui| viewer.show(ui));
    output.textures_delta.clear();
    output
}

/// Where a piece of text was drawn in a frame.
fn text_rect(output: &egui::FullOutput, text: &str) -> Rect {
    output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(shape) if shape.galley.text() == text => {
                Some(shape.galley.rect.translate(shape.pos.to_vec2()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("nothing says {text}"))
}

/// Where the open flyout is: the one area in front of the window's own.
fn flyout_rect(ctx: &egui::Context) -> Rect {
    ctx.memory(|memory| {
        let areas = memory.areas();
        let mut flyouts = areas
            .visible_layer_ids()
            .into_iter()
            .filter(|layer| layer.order == egui::Order::Foreground)
            .filter_map(|layer| memory.area_rect(layer.id));
        let rect = flyouts.next().expect("a flyout is open");
        assert!(flyouts.next().is_none(), "only one flyout is open");
        rect
    })
}

#[test]
fn the_top_bar_reads_info_trim_edit_in_bareclip_delete() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let nodes = named(&ctx, &mut viewer);
    let left_of = |name| rect_of(&nodes, name).left();
    assert!(left_of("Info") < left_of("Trim"));
    assert!(left_of("Trim") < left_of("Edit in BareClip"));
    assert!(left_of("Edit in BareClip") < left_of("Delete"));
    assert_eq!(
        rect_of(&nodes, "Edit in BareClip").size(),
        rect_of(&nodes, "Trim").size()
    );
}

#[test]
fn volume_zoom_speed_mini_view_and_full_screen_are_one_size() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let nodes = named(&ctx, &mut viewer);
    let size = rect_of(&nodes, "Full screen").size();
    for name in ["Volume", "Zoom", "Watching speed", "Mini view"] {
        assert_eq!(rect_of(&nodes, name).size(), size, "{name}");
    }
    assert_eq!(
        rect_of(&nodes, "Jump back").size(),
        rect_of(&nodes, "Jump forward").size()
    );
}

#[test]
fn home_carries_on_from_the_start_and_end_pauses_on_the_last_frame() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    viewer.seek(seconds(2.0));
    viewer.play();
    frame(&ctx, &mut viewer, press(Key::Home));
    assert_eq!(viewer.position(), Ticks(0));
    assert!(viewer.is_playing(), "going to the start keeps playing");
    frame(&ctx, &mut viewer, press(Key::End));
    assert!(!viewer.is_playing(), "the end is shown paused");
    assert!(near(viewer.position(), viewer.duration(), 0.001));
}

#[test]
fn each_flyout_opens_with_its_middle_over_its_button_which_shows_it_is_open() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let middle_x = |rect: Rect| rect.center().x;

    rest_on(&ctx, &mut viewer, "Volume");
    assert_eq!(viewer.flyout.open, Some(bars::Flyout::Volume));
    assert_eq!(
        bars::flyout_kind(viewer.flyout.open, bars::Flyout::Volume),
        Kind::Chosen,
        "its button shows as held in"
    );
    assert_eq!(
        bars::flyout_kind(viewer.flyout.open, bars::Flyout::Zoom),
        Kind::Ghost
    );
    settle(&ctx, &mut viewer);
    let nodes = named(&ctx, &mut viewer);
    let row = flyout_rect(&ctx);
    let button = rect_of(&nodes, "Volume");
    assert!(
        (middle_x(row) - middle_x(button)).abs() <= 1.0,
        "the volume's flyout is centred on it: {} and {}",
        middle_x(row),
        middle_x(button)
    );

    // Opening another closes the first.
    rest_on(&ctx, &mut viewer, "Watching speed");
    assert_eq!(viewer.flyout.open, Some(bars::Flyout::Speed));
    settle(&ctx, &mut viewer);
    let nodes = named(&ctx, &mut viewer);
    assert!(!nodes.iter().any(|(label, ..)| label == "Volume level"));
    assert!(
        (middle_x(flyout_rect(&ctx)) - middle_x(rect_of(&nodes, "Watching speed"))).abs() <= 1.0,
        "the speed menu is centred on its button: {:?} and {:?}",
        flyout_rect(&ctx),
        rect_of(&nodes, "Watching speed")
    );

    rest_on(&ctx, &mut viewer, "Zoom");
    assert_eq!(viewer.flyout.open, Some(bars::Flyout::Zoom));
    settle(&ctx, &mut viewer);
    let nodes = named(&ctx, &mut viewer);
    let reset = rect_of(&nodes, "Reset zoom");
    let row = flyout_rect(&ctx);
    assert!(
        (middle_x(row) - middle_x(rect_of(&nodes, "Zoom"))).abs() <= 1.0,
        "the zoom's flyout is centred on it"
    );
    assert!(reset.bottom() < rect_of(&nodes, "Zoom").top());

    // A click elsewhere closes it.
    click_at(&ctx, &mut viewer, ON_THE_PICTURE);
    assert_eq!(viewer.flyout.open, None);
}

#[test]
fn a_flyout_waits_for_the_mouse_to_rest_and_goes_a_moment_after_it_leaves() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let volume = named_rect(&ctx, &mut viewer, "Volume");
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(volume.center())],
    );
    frame(&ctx, &mut viewer, vec![]);
    assert_eq!(viewer.flyout.open, None, "passing over opens nothing");
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(ON_THE_PICTURE)],
    );
    viewer.flyout.rest_is_up();
    frame(&ctx, &mut viewer, vec![]);
    assert_eq!(
        viewer.flyout.open, None,
        "nor does a rest that ended before it was up"
    );

    rest_on(&ctx, &mut viewer, "Volume");
    assert_eq!(viewer.flyout.open, Some(bars::Flyout::Volume));
    settle(&ctx, &mut viewer);
    let on_it = flyout_rect(&ctx).center();
    frame(&ctx, &mut viewer, vec![egui::Event::PointerMoved(on_it)]);
    assert!(!viewer.flyout.is_closing(), "the mouse on it keeps it");

    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(ON_THE_PICTURE)],
    );
    assert!(viewer.flyout.is_closing(), "the mouse away starts it going");
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(volume.center())],
    );
    assert!(
        !viewer.flyout.is_closing(),
        "and coming back in time keeps it"
    );
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(ON_THE_PICTURE)],
    );
    viewer.flyout.away_is_up();
    frame(&ctx, &mut viewer, vec![]);
    assert_eq!(viewer.flyout.open, None, "a moment away and it goes");

    // Closed while the mouse rests on its button, it waits for the mouse to come back.
    rest_on(&ctx, &mut viewer, "Volume");
    frame(&ctx, &mut viewer, press(Key::Escape));
    viewer.flyout.rest_is_up();
    frame(&ctx, &mut viewer, vec![]);
    assert_eq!(viewer.flyout.open, None, "Escape keeps it shut");
    rest_on(&ctx, &mut viewer, "Volume");
    assert_eq!(viewer.flyout.open, Some(bars::Flyout::Volume));

    // A held mouse button keeps it, as while dragging the slider past its edge.
    settle(&ctx, &mut viewer);
    let slider = named_rect(&ctx, &mut viewer, "Volume level");
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(slider.center())],
    );
    frame(&ctx, &mut viewer, vec![button(slider.center(), true)]);
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(ON_THE_PICTURE)],
    );
    assert!(!viewer.flyout.is_closing(), "held, it stays");
    frame(&ctx, &mut viewer, vec![button(ON_THE_PICTURE, false)]);
    frame(&ctx, &mut viewer, vec![]);
    assert!(
        viewer.flyout.is_closing(),
        "let go away from it, it starts going"
    );
}

#[test]
fn a_click_on_speed_or_zoom_puts_it_back_to_normal() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    frame(&ctx, &mut viewer, vec![]);
    let fit = viewer.zoom_percent(&ctx);
    viewer.set_speed(5);
    viewer.zoom_to(&ctx, 250.0);
    frame(&ctx, &mut viewer, vec![]);
    click_named(&ctx, &mut viewer, "Watching speed");
    assert_eq!(viewer.speed(), 1.0, "back to 1x");
    click_named(&ctx, &mut viewer, "Zoom");
    assert!(
        (viewer.zoom_percent(&ctx) - fit).abs() < 0.5,
        "back to fit: {}",
        viewer.zoom_percent(&ctx)
    );
    assert_eq!(viewer.flyout.open, None, "a quick click opens nothing");
}

/// How much the open flyout is scaled as it grows out of its button, if it is.
fn flyout_scaling(ctx: &egui::Context) -> Option<f32> {
    let layers = ctx.memory(|memory| memory.areas().visible_layer_ids());
    layers
        .into_iter()
        .filter(|layer| layer.order == egui::Order::Foreground)
        .find_map(|layer| ctx.layer_transform_to_global(layer))
        .map(|transform| transform.scaling)
}

#[test]
fn a_flyout_grows_out_of_its_button_as_it_opens() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    rest_on(&ctx, &mut viewer, "Volume");
    frame(&ctx, &mut viewer, vec![]);
    let early = flyout_scaling(&ctx).expect("it starts smaller");
    assert!(early < 1.0, "{early}");
    settle(&ctx, &mut viewer);
    assert_eq!(flyout_scaling(&ctx), None, "and ends at its full size");
}

#[test]
fn the_pill_sinks_away_when_the_mouse_rests_and_comes_back_when_it_moves() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let play = named_rect(&ctx, &mut viewer, "Play");
    viewer.play();
    frame(&ctx, &mut viewer, vec![]);
    rest_for(&mut viewer, PILL_LINGER);
    frame(&ctx, &mut viewer, vec![]);
    let going = named_rect(&ctx, &mut viewer, "Pause");
    assert!(going.top() > play.top(), "it sinks as it goes");
    assert_eq!(going.left(), play.left(), "straight down");
    settle(&ctx, &mut viewer);
    assert!(
        !named(&ctx, &mut viewer)
            .iter()
            .any(|(label, ..)| label == "Pause"),
        "gone once out of sight"
    );
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(300.0, 300.0))],
    );
    settle(&ctx, &mut viewer);
    assert_eq!(
        named_rect(&ctx, &mut viewer, "Pause"),
        play,
        "back in place once the mouse moves"
    );
}

#[test]
fn the_seek_line_has_the_same_gap_to_each_time() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let output = frame(&ctx, &mut viewer, vec![]);
    let start = text_rect(&output, &viewer.clock(Ticks(0)));
    let end = text_rect(&output, &viewer.clock(viewer.duration()));
    let pill = viewer.pill_at;
    assert!(
        pill.contains(start.center()) && pill.contains(end.center()),
        "on the pill"
    );
    let seek = named_rect(&ctx, &mut viewer, "Seek");
    let (before, after) = (seek.left() - start.right(), end.left() - seek.right());
    assert!(
        (before - after).abs() <= 1.0,
        "{before} before the line, {after} after it"
    );
}

#[test]
fn the_wheel_shows_its_flyout_until_the_mouse_has_been_away_a_moment() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    frame(&ctx, &mut viewer, vec![]);
    let middle = viewer.view.center();
    frame(&ctx, &mut viewer, wheel(middle, 1.0));
    assert_eq!(
        viewer.flyout.open,
        Some(bars::Flyout::Zoom),
        "zooming shows the zoom"
    );
    frame(&ctx, &mut viewer, vec![]);
    assert!(viewer.flyout.is_closing(), "the mouse is on the picture");
    viewer.flyout.away_is_up();
    frame(&ctx, &mut viewer, vec![]);
    assert_eq!(viewer.flyout.open, None, "it goes by itself");

    // Moving onto it keeps it.
    frame(&ctx, &mut viewer, wheel(middle, -1.0));
    settle(&ctx, &mut viewer);
    let on_it = flyout_rect(&ctx).center();
    frame(&ctx, &mut viewer, vec![egui::Event::PointerMoved(on_it)]);
    assert!(!viewer.flyout.is_closing());
    assert_eq!(viewer.flyout.open, Some(bars::Flyout::Zoom), "it stays");
    frame(&ctx, &mut viewer, press(Key::Escape));

    // The wheel over the volume shows the volume's slider, which stays while the mouse does.
    let volume = named_rect(&ctx, &mut viewer, "Volume");
    let zoom_before = viewer.zoom;
    frame(&ctx, &mut viewer, wheel(volume.center(), -1.0));
    assert_eq!(
        viewer.zoom, zoom_before,
        "the wheel over a bar doesn't zoom the picture under it"
    );
    assert_eq!(viewer.flyout.open, Some(bars::Flyout::Volume));
    frame(&ctx, &mut viewer, vec![]);
    assert!(!viewer.flyout.is_closing());
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(ON_THE_PICTURE)],
    );
    viewer.flyout.away_is_up();
    frame(&ctx, &mut viewer, vec![]);
    assert_eq!(viewer.flyout.open, None);
}

#[test]
fn playing_pausing_or_moving_through_closes_a_flyout() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    for key in [Key::Space, Key::K, Key::ArrowRight, Key::Home, Key::Period] {
        rest_on(&ctx, &mut viewer, "Watching speed");
        assert!(viewer.flyout.is_open());
        frame(&ctx, &mut viewer, press(key));
        assert!(!viewer.flyout.is_open(), "{key:?} closes it");
    }
    rest_on(&ctx, &mut viewer, "Volume");
    frame(&ctx, &mut viewer, press(Key::ArrowUp));
    assert_eq!(
        viewer.flyout.open,
        Some(bars::Flyout::Volume),
        "turning the volume up leaves its slider open"
    );
}

#[test]
fn the_status_bar_is_one_row_along_the_bottom() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let full = named_rect(&ctx, &mut viewer, "Full screen");
    let bottom = screen_camera::screen().bottom();
    assert_eq!(full.center().y, bottom - bars::BOTTOM_HEIGHT / 2.0);
}

#[test]
fn full_screen_from_maximised_unmaximises_first_and_maximises_again_after() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let sent = commands(&frame_in_window(
        &ctx,
        &mut viewer,
        true,
        false,
        press(Key::F),
    ));
    assert!(viewer.is_full_screen());
    let at = |wanted: &egui::ViewportCommand| sent.iter().position(|command| command == wanted);
    let unmaximise = at(&egui::ViewportCommand::Maximized(false)).expect("it unmaximises");
    let full = at(&egui::ViewportCommand::Fullscreen(true)).expect("it goes full screen");
    assert!(
        unmaximise < full,
        "a maximised window would stop at the taskbar"
    );

    let sent = commands(&frame_in_window(
        &ctx,
        &mut viewer,
        false,
        true,
        press(Key::Escape),
    ));
    let at = |wanted: &egui::ViewportCommand| sent.iter().position(|command| command == wanted);
    let leave = at(&egui::ViewportCommand::Fullscreen(false)).expect("it leaves full screen");
    let maximise = at(&egui::ViewportCommand::Maximized(true)).expect("and is maximised again");
    assert!(leave < maximise);

    // From a window that wasn't maximised, nothing is maximised on the way out.
    frame_in_window(&ctx, &mut viewer, false, false, press(Key::F));
    let sent = commands(&frame_in_window(
        &ctx,
        &mut viewer,
        false,
        true,
        press(Key::Escape),
    ));
    assert!(!sent.contains(&egui::ViewportCommand::Maximized(true)));
}

#[test]
fn maximise_in_full_screen_leaves_full_screen_instead() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    frame(&ctx, &mut viewer, press(Key::F));
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(300.0, 10.0))],
    );
    settle(&ctx, &mut viewer);
    let sent = click_named_for_commands(&ctx, &mut viewer, "Maximise window");
    assert!(!viewer.is_full_screen());
    assert!(!sent.contains(&egui::ViewportCommand::Maximized(true)));
}

#[test]
fn closing_in_full_screen_keeps_the_window_place_from_before() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let before = WindowPlace {
        left: 100,
        top: 100,
        right: 900,
        bottom: 600,
        maximized: false,
    };
    let whole_screen = WindowPlace {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
        maximized: false,
    };
    viewer.full_screen = true;
    viewer.before_full_screen = Some((false, Some(before)));
    viewer.closing(Some(whole_screen));
    assert_eq!(viewer.saved_window(), Some(before));
}

#[test]
fn the_window_edges_resize_only_in_a_plain_window() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let edge = pos2(1.0, 400.0);
    let press_edge = || {
        vec![
            egui::Event::PointerMoved(edge),
            egui::Event::PointerButton {
                pos: edge,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]
    };
    let resizes = |sent: Vec<egui::ViewportCommand>| {
        sent.iter()
            .any(|command| matches!(command, egui::ViewportCommand::BeginResize(_)))
    };
    let release = || {
        vec![egui::Event::PointerButton {
            pos: edge,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]
    };
    assert!(resizes(commands(&frame_in_window(
        &ctx,
        &mut viewer,
        false,
        false,
        press_edge()
    ))));
    frame_in_window(&ctx, &mut viewer, false, false, release());
    for (maximized, fullscreen) in [(true, false), (false, true)] {
        assert!(
            !resizes(commands(&frame_in_window(
                &ctx,
                &mut viewer,
                maximized,
                fullscreen,
                press_edge()
            ))),
            "maximised {maximized}, full screen {fullscreen}"
        );
        frame_in_window(&ctx, &mut viewer, maximized, fullscreen, release());
    }
}

#[test]
fn the_trim_line_and_its_times_stay_put_as_the_cut_moves() {
    let ctx = context();
    let mut viewer = trimmer_on(&ctx, video());
    let line = named_rect(&ctx, &mut viewer, "Seek");
    let start_text = |viewer: &mut Viewer| {
        let output = frame(&ctx, viewer, vec![]);
        let start = viewer.clock(viewer.trim_range().unwrap().start);
        // The lowest, which is the one in the pill rather than the bubble over it.
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape) if shape.galley.text() == start => {
                    Some(shape.galley.rect.translate(shape.pos.to_vec2()))
                }
                _ => None,
            })
            .max_by(|a, b| a.top().total_cmp(&b.top()))
            .expect("the start time shows")
    };
    let before = start_text(&mut viewer);
    viewer.set_trim_start(seconds(1.23));
    viewer.set_trim_end(seconds(2.5));
    assert_eq!(
        named_rect(&ctx, &mut viewer, "Seek"),
        line,
        "the line doesn't move"
    );
    let after = start_text(&mut viewer);
    assert!(
        (after.right() - before.right()).abs() <= 0.5,
        "the start time keeps its place: {before:?} then {after:?}"
    );
}

#[test]
fn f11_also_toggles_full_screen_which_hides_the_shadows_gap_and_doesnt_drag() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    let sent = commands(&frame(&ctx, &mut viewer, press(Key::F11)));
    assert!(viewer.is_full_screen(), "F11 goes into full screen");
    let at = |sent: &[egui::ViewportCommand], wanted: &egui::ViewportCommand| {
        sent.iter().position(|command| command == wanted)
    };
    let full = at(&sent, &egui::ViewportCommand::Fullscreen(true)).expect("full screen");
    let shadow = at(&sent, &egui::ViewportCommand::Decorations(false))
        .expect("the window's shadow, and the pixel it moves things by, are turned off");
    assert!(full < shadow, "after going full screen");

    // The top bar doesn't move or maximise the window in full screen.
    let title = pos2(300.0, bars::TITLE_HEIGHT / 2.0);
    let press_at = |pressed| egui::Event::PointerButton {
        pos: title,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    let mut dragged = Vec::new();
    for events in [
        vec![egui::Event::PointerMoved(title)],
        vec![press_at(true)],
        vec![egui::Event::PointerMoved(title + vec2(40.0, 10.0))],
        vec![press_at(false)],
    ] {
        dragged.extend(commands(&frame(&ctx, &mut viewer, events)));
    }
    assert!(
        !dragged.contains(&egui::ViewportCommand::StartDrag),
        "no dragging in full screen"
    );

    let sent = commands(&frame(&ctx, &mut viewer, press(Key::F11)));
    assert!(!viewer.is_full_screen(), "F11 leaves it");
    let leave = at(&sent, &egui::ViewportCommand::Fullscreen(false)).expect("leaves");
    let shadow = at(&sent, &egui::ViewportCommand::Decorations(false)).expect("shadow back");
    assert!(leave < shadow);
}

#[test]
fn the_info_card_slides_in_from_the_right_narrowing_the_picture_and_slides_back_out() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    settle(&ctx, &mut viewer);
    let whole = viewer.view;
    click_named(&ctx, &mut viewer, "Info");
    assert!(viewer.is_info_open());
    let partway = viewer.view;
    let open_right = whole.right() - info::CARD_WIDTH - info::CARD_MARGIN * 2.0;
    assert!(
        partway.right() < whole.right() && partway.right() > open_right,
        "the picture narrows as the card comes: {partway:?}"
    );
    settle(&ctx, &mut viewer);
    assert!((viewer.view.right() - open_right).abs() < 0.5);
    assert_eq!(
        viewer.view.top(),
        whole.top(),
        "the card sits between the bars"
    );
    assert_eq!(viewer.view.bottom(), whole.bottom());

    // Copy path, Open location and × are symbols on the heading's line, at its right.
    let output = frame(&ctx, &mut viewer, vec![]);
    let heading = text_rect(&output, "Info");
    let nodes = named(&ctx, &mut viewer);
    let (copy, open, close) = (
        rect_of(&nodes, "Copy path"),
        rect_of(&nodes, "Open location"),
        rect_of(&nodes, "Close info"),
    );
    for button in [copy, open, close] {
        assert!(
            (button.center().y - heading.center().y).abs() <= 2.0,
            "on the heading's line"
        );
        assert_eq!(button.width(), button.height(), "a symbol, not words");
    }
    assert!(heading.right() < copy.left() && copy.right() < open.left());
    assert!(open.right() < close.left());
    assert!(
        close.right() <= whole.right() - info::CARD_MARGIN,
        "inside the card"
    );

    click_named(&ctx, &mut viewer, "Close info");
    assert!(!viewer.is_info_open());
    assert!(viewer.info.is_some(), "it is still sliding away");
    assert!(viewer.view.right() < whole.right());
    settle(&ctx, &mut viewer);
    assert!(viewer.info.is_none(), "and goes once out of sight");
    assert_eq!(viewer.view, whole);
}

#[test]
fn changeable_facts_are_one_line_like_the_others_and_turn_into_a_box_in_place() {
    let ctx = context();
    let folder = folder_of_videos("info-rows", &["clip.mp4"]);
    let mut viewer = opened(&ctx, &folder.join("clip.mp4"));
    viewer.toggle_info();
    wait_for_details(&ctx, &mut viewer);
    settle(&ctx, &mut viewer);
    let output = frame(&ctx, &mut viewer, vec![]);
    let name_text = text_rect(&output, "clip.mp4");
    let size_name = text_rect(&output, "Size");
    let nodes = named(&ctx, &mut viewer);
    for name in [
        "File name",
        "Created",
        "Modified",
        info::MADE_LABEL,
        "Artist",
    ] {
        let row = rect_of(&nodes, name);
        assert!(row.height() <= 20.0, "{name} is one line: {row:?}");
    }
    let row = rect_of(&nodes, "File name");
    assert!(
        row.contains(name_text.center()),
        "the name shows with its extension, on its line"
    );
    assert!(
        (row.left() - size_name.left()).abs() <= 1.0,
        "it lines up with the facts that can't change"
    );
    assert!(
        name_text.left() - row.left() >= 100.0,
        "and its value sits in the values' column"
    );

    click_named(&ctx, &mut viewer, "File name");
    let output = frame(&ctx, &mut viewer, vec![]);
    let typing = named_rect(&ctx, &mut viewer, "File name");
    assert!(
        (typing.center().y - row.center().y).abs() <= 3.0,
        "the box is on the same line"
    );
    assert!(viewer.info.as_ref().unwrap().is_typing());
    let extension = text_rect(&output, ".mp4");
    assert!(
        extension.left() >= typing.right(),
        "the extension stays after the box"
    );
    assert_eq!(
        text_rect(&output, "Size").top(),
        size_name.top(),
        "nothing below moves when the box opens"
    );
    frame(&ctx, &mut viewer, press(Key::Escape));
    frame(&ctx, &mut viewer, vec![]);
    assert!(
        !viewer.info.as_ref().unwrap().is_typing(),
        "Escape puts it back"
    );
}

#[test]
fn closing_the_panel_or_going_full_screen_partway_through_a_change_still_opens_the_file_again() {
    let ctx = context();
    for full_screen in [false, true] {
        let folder = folder_of_videos(&format!("info-close-{full_screen}"), &["clip.mp4"]);
        let path = folder.join("clip.mp4");
        let mut viewer = opened(&ctx, &path);
        viewer.toggle_info();
        wait_for_details(&ctx, &mut viewer);
        viewer.change_tags(
            &ctx,
            bareclip_media::TagChanges {
                artist: Some("Somebody".to_owned()),
                creation_time: None,
            },
        );
        if full_screen {
            frame(&ctx, &mut viewer, press(Key::F));
        } else {
            viewer.toggle_info();
        }
        wait_for(&ctx, &mut viewer, "the file to come back", |viewer| {
            viewer.file.is_some() && !viewer.is_rewriting()
        });
        let details = bareclip_media::read_details(&path).unwrap();
        assert_eq!(
            details.tag("artist"),
            Some("Somebody"),
            "full screen {full_screen}"
        );
        if !full_screen {
            settle(&ctx, &mut viewer);
            assert!(viewer.info.is_none(), "the closed panel goes after");
        }
    }
}

#[test]
fn the_trimmers_more_sets_the_ends_at_the_playhead_and_resets_them() {
    let ctx = context();
    let mut viewer = trimmer_on(&ctx, video());
    let one = 1.0 / 30.0;
    let choose = |viewer: &mut Viewer, name: &str| {
        click_named(&ctx, viewer, "More options");
        settle(&ctx, viewer);
        click_named(&ctx, viewer, name);
        settle(&ctx, viewer);
    };
    viewer.seek(during_frame(30));
    choose(&mut viewer, "Start at playhead");
    viewer.seek(during_frame(60));
    choose(&mut viewer, "End at playhead");
    let range = viewer.trim_range().unwrap();
    assert!(
        near(range.start, seconds(30.0 * one), 0.0005),
        "start at the playhead"
    );
    assert!(
        near(range.end, seconds(61.0 * one), 0.0005),
        "end after its frame"
    );
    choose(&mut viewer, "Reset to whole clip");
    let range = viewer.trim_range().unwrap();
    assert_eq!((range.start, range.end), (Ticks(0), viewer.duration()));
}

#[test]
fn loop_the_kept_part_goes_round_between_the_handles() {
    let ctx = context();
    let mut viewer = trimmer_on(&ctx, video());
    viewer.set_trim_start(seconds(1.0));
    viewer.set_trim_end(seconds(1.4));
    click_named(&ctx, &mut viewer, "More options");
    settle(&ctx, &mut viewer);
    click_named(&ctx, &mut viewer, "Loop the kept part");
    assert!(viewer.trim.as_ref().unwrap().looping);
    viewer.seek(seconds(1.0));
    viewer.toggle_playback();
    let started = Instant::now();
    let mut went_round = false;
    let mut furthest = Ticks(0);
    while started.elapsed() < Duration::from_secs(5) {
        frame(&ctx, &mut viewer, vec![]);
        let at = viewer.position();
        if furthest > seconds(1.3) && at < seconds(1.15) {
            went_round = true;
            break;
        }
        furthest = furthest.max(at);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(went_round, "it went back to the start handle");
    assert!(viewer.is_playing(), "and carries on playing");
}

#[test]
fn save_without_sound_leaves_the_sound_behind_copied_or_redrawn() {
    let ctx = context();
    let folder = temporary_folder("trim-silent");
    for (name, codecs) in [
        ("clip.mp4", &["-c:v", "libopenh264", "-c:a", "aac"][..]),
        ("clip.wmv", &["-c:v", "wmv2", "-c:a", "wmav2"][..]),
    ] {
        let path = folder.join(name);
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc2=s=160x90:r=30:d=2"])
            .args(["-f", "lavfi", "-i", "sine=frequency=330:duration=2"])
            .args(codecs)
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let mut viewer = trimmer_on(&ctx, &path);
        assert!(
            !viewer.file.as_ref().unwrap().info.audio.is_empty(),
            "{name} has sound"
        );
        viewer.set_trim_start(seconds(0.5));
        viewer.set_trim_end(seconds(1.5));
        click_named(&ctx, &mut viewer, "More options");
        settle(&ctx, &mut viewer);
        click_named(&ctx, &mut viewer, "Save without sound");
        assert!(viewer.trim.as_ref().unwrap().without_sound);
        viewer.save_trim(&ctx, None);
        let saved = wait_for_save(&ctx, &mut viewer);
        let info = bareclip_media::probe(&saved).unwrap();
        assert!(
            info.audio.is_empty(),
            "{name}: no sound in {}",
            saved.display()
        );
        assert_eq!((info.video.width, info.video.height), (160, 90), "{name}");
        assert!(
            (info.duration.as_seconds() - 1.0).abs() < 0.15,
            "{name}: {}",
            info.duration.as_seconds()
        );
    }

    // A song has nothing to leave behind but its sound.
    let mut viewer = trimmer_on(&ctx, &make_song("trim-silent-song", false));
    click_named(&ctx, &mut viewer, "More options");
    settle(&ctx, &mut viewer);
    assert!(
        !named(&ctx, &mut viewer)
            .iter()
            .any(|(label, ..)| label == "Save without sound")
    );
}

#[test]
fn the_trimmers_playhead_is_a_thumb_on_the_line_that_drags() {
    let ctx = context();
    let mut viewer = trimmer_on(&ctx, video());
    viewer.set_trim_start(seconds(1.0));
    viewer.set_trim_end(seconds(3.0));
    viewer.seek(seconds(2.0));
    let seek = named_rect(&ctx, &mut viewer, "Seek");
    let duration = viewer.duration().as_seconds() as f32;
    // The seeking stretch is the track with the thumb's widest added at each end.
    let x_at = |time: f32| {
        let left = seek.left() + 8.0;
        let width = seek.width() - 16.0;
        left + width * time / duration
    };
    let thumb = pos2(x_at(2.0), seek.center().y);
    let target = pos2(x_at(1.5), seek.center().y);
    let button = |at, pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    for events in [
        vec![egui::Event::PointerMoved(thumb)],
        vec![button(thumb, true)],
        vec![egui::Event::PointerMoved(thumb + vec2(-10.0, 0.0))],
        vec![egui::Event::PointerMoved(target)],
        vec![button(target, false)],
    ] {
        frame(&ctx, &mut viewer, events);
    }
    assert!(
        near(viewer.position(), seconds(1.5), 0.1),
        "dragged to {}",
        viewer.position().as_seconds()
    );
    let range = viewer.trim_range().unwrap();
    assert!(
        near(range.start, seconds(1.0), 0.04) && near(range.end, seconds(3.0), 0.04),
        "the handles stay"
    );
}

#[test]
fn edit_in_bareclip_sends_the_whole_file_to_an_open_editor_and_pauses() {
    let ctx = context();
    let prefix = format!("ViewerEditTest{}", std::process::id());
    let editor = crate::handoff::starting_editor(&prefix);
    let mut viewer = opened(&ctx, video());
    viewer.editor_prefix = prefix;
    viewer.play();
    click_named(&ctx, &mut viewer, "Edit in BareClip");

    let started = Instant::now();
    let mut arrived = Vec::new();
    while arrived.is_empty() && started.elapsed() < Duration::from_secs(10) {
        arrived = editor.take();
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(arrived.len(), 1);
    assert_eq!(arrived[0].path, video());
    assert_eq!(
        arrived[0].range,
        Some(TimeRange {
            start: Ticks(0),
            end: viewer.duration()
        })
    );
    assert!(!viewer.is_playing(), "the player pauses and stays");
    assert!(viewer.take_become_editor().is_none());
}

/// Whether resting the mouse on a control shows exactly these words, with no wait.
fn hint_shows(ctx: &egui::Context, viewer: &mut Viewer, name: &str, hint: &str) -> bool {
    ctx.all_styles_mut(|style| {
        style.interaction.tooltip_delay = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
    });
    let at = named_rect(ctx, viewer, name).center();
    frame(ctx, viewer, vec![egui::Event::PointerMoved(at)]);
    // Moving straight from one hint to the next takes a frame more to show it.
    frame(ctx, viewer, vec![]);
    let output = frame(ctx, viewer, vec![]);
    output.shapes.iter().any(|clipped| {
        matches!(&clipped.shape, egui::epaint::Shape::Text(shape) if shape.galley.text() == hint)
    })
}

#[test]
fn the_players_hints_are_short_and_play_says_what_a_click_does() {
    screen_camera::SCREEN.set([1280, 720]);
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    for (name, hint) in [
        ("Trim", "Trim"),
        ("Delete", "Delete"),
        ("Play", "Play"),
        ("Jump back", "Back 5s"),
        ("Jump forward", "Forward 5s"),
        ("Save frame", "Save frame"),
        ("Mini view", "Mini view"),
        ("Full screen", "Fullscreen (F)"),
    ] {
        assert!(
            hint_shows(&ctx, &mut viewer, name, hint),
            "{name} says {hint}"
        );
    }
    assert!(!hint_shows(
        &ctx,
        &mut viewer,
        "Play",
        "Play or pause (Space)"
    ));

    viewer.set_jump(false, 10);
    viewer.set_jump(true, 30);
    assert!(hint_shows(&ctx, &mut viewer, "Jump back", "Back 10s"));
    assert!(hint_shows(&ctx, &mut viewer, "Jump forward", "Forward 30s"));

    viewer.toggle_playback();
    assert!(viewer.is_playing());
    assert!(hint_shows(&ctx, &mut viewer, "Pause", "Pause"));
    viewer.pause();

    // The trimmer's play button says the same.
    let mut viewer = trimmer_on(&ctx, video());
    assert!(hint_shows(&ctx, &mut viewer, "Play", "Play"));
}

#[test]
fn settings_ticks_the_default_player_and_asks_again_on_coming_back() {
    screen_camera::SCREEN.set([1280, 720]);
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    viewer.is_default_player = || true;
    viewer.settings_open = true;
    settle(&ctx, &mut viewer);
    let output = frame(&ctx, &mut viewer, vec![]);
    for words in ["Jump back by", "Jump forward by", DEFAULT_PLAYER_NOTE] {
        assert!(text_rect(&output, words).is_positive(), "{words}");
    }
    let shows_default = |output: &egui::FullOutput| {
        output.shapes.iter().any(|clipped| {
            matches!(
                &clipped.shape,
                egui::epaint::Shape::Text(shape)
                    if shape.galley.text() == "Default"
                        && shape.fallback_color == theme::palette(&ctx).text
            )
        })
    };
    assert!(
        shows_default(&output),
        "a tick and Default in place of the button, in the text's own colour"
    );
    let nodes = named(&ctx, &mut viewer);
    assert!(!nodes.iter().any(|(label, ..)| label == "Make default"));
    for gone in [
        "Jump back and the Left key",
        "Jump forward and the Right key",
    ] {
        assert!(
            !output.shapes.iter().any(|clipped| matches!(
                &clipped.shape,
                egui::epaint::Shape::Text(shape) if shape.galley.text() == gone
            )),
            "{gone} is gone"
        );
    }

    // Changed in Windows while Settings stayed open: nothing is asked until the window is back.
    viewer.is_default_player = || false;
    assert!(shows_default(&frame(&ctx, &mut viewer, vec![])));
    frame(&ctx, &mut viewer, vec![egui::Event::WindowFocused(true)]);
    let output = frame(&ctx, &mut viewer, vec![]);
    assert!(text_rect(&output, NOT_DEFAULT_PLAYER_NOTE).is_positive());
    assert!(!shows_default(&output));
    let nodes = named(&ctx, &mut viewer);
    assert!(nodes.iter().any(|(label, ..)| label == "Make default"));

    // Closing Settings forgets the answer, so opening it again asks afresh.
    viewer.settings_open = false;
    frame(&ctx, &mut viewer, vec![]);
    assert_eq!(viewer.default_player, None);
    viewer.is_default_player = || true;
    viewer.settings_open = true;
    settle(&ctx, &mut viewer);
    assert!(shows_default(&frame(&ctx, &mut viewer, vec![])));
}

#[test]
fn in_the_smallest_window_the_trimmers_bars_fit() {
    let ctx = context();
    let mut viewer = trimmer_on(&ctx, video());
    for size in [[640, 340], [900, 600], [1920, 1080]] {
        screen_camera::SCREEN.set(size);
        settle(&ctx, &mut viewer);
        let nodes = named(&ctx, &mut viewer);
        let screen = screen_camera::screen();
        let (save, cancel) = (rect_of(&nodes, trim::SAVE_AS), rect_of(&nodes, "Cancel"));
        let info = rect_of(&nodes, "Info");
        let minimise = rect_of(&nodes, "Minimise window");
        assert_eq!(save.size(), cancel.size(), "{size:?}: the same size");
        assert!(
            cancel.right() < info.left(),
            "{size:?}: Cancel clear of Info"
        );
        assert!(
            info.right() < minimise.left(),
            "{size:?}: Info clear of the window's buttons"
        );
        let (play, more) = (rect_of(&nodes, "Play"), rect_of(&nodes, "More options"));
        assert!(
            play.left() >= screen.left(),
            "{size:?}: the pill starts on screen"
        );
        assert!(more.right() <= screen.right(), "{size:?}: and ends on it");
        let line = rect_of(&nodes, "Trim end").left() - rect_of(&nodes, "Trim start").left();
        assert!(line > 0.0, "{size:?}: the handles have a line between them");
    }
    screen_camera::SCREEN.set([1280, 720]);
}

#[test]
fn trimming_keeps_to_the_file_on_screen() {
    screen_camera::SCREEN.set([1280, 720]);
    let ctx = context();
    let folder = folder_of_videos("trim-stays", &["a.mp4", "b.mp4", "c.mp4"]);
    let middle = folder.join("b.mp4");
    let mut viewer = trimmer_on(&ctx, &middle);
    wait_for(&ctx, &mut viewer, "the folder", |viewer| {
        viewer.next_file().is_some() && viewer.previous_file().is_some()
    });
    // The mouse over the picture, where the arrows would show.
    let middle_of_picture = viewer.view.center();
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(middle_of_picture)],
    );
    let nodes = named(&ctx, &mut viewer);
    for arrow in ["Previous file", "Next file"] {
        assert!(
            !nodes.iter().any(|(label, ..)| label == arrow),
            "no {arrow} while trimming"
        );
    }
    for key in [Key::PageDown, Key::PageUp, Key::Delete] {
        frame(&ctx, &mut viewer, press(key));
        frame(&ctx, &mut viewer, vec![]);
        assert!(viewer.is_trimming(), "{key:?} leaves the trimmer open");
        assert_eq!(viewer.current_path(), Some(middle.as_path()), "{key:?}");
        assert!(!viewer.confirm_delete, "{key:?} asks nothing");
    }

    // Once the trimmer is closed they work again.
    viewer.close_trimmer();
    frame(&ctx, &mut viewer, press(Key::PageDown));
    assert_eq!(viewer.current_path(), Some(folder.join("c.mp4").as_path()));
}

#[test]
fn the_space_around_the_picture_is_the_windows_own_colour() {
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.set_theme(theme);
        let palette = theme::palette(&ctx);
        for full_screen in [false, true] {
            viewer.full_screen = full_screen;
            frame(&ctx, &mut viewer, vec![]);
            let output = frame(&ctx, &mut viewer, vec![]);
            let filled = |colour: Color32| {
                output.shapes.iter().any(|clipped| {
                    matches!(
                        &clipped.shape,
                        egui::epaint::Shape::Rect(shape)
                            if shape.fill == colour && shape.rect == viewer.view
                    )
                })
            };
            assert!(
                filled(palette.bg_app),
                "{theme:?}, full screen {full_screen}: the picture's space is the window's colour"
            );
            assert!(!filled(palette.stage), "{theme:?}: and not the dark stage");
        }
    }
    viewer.full_screen = false;
}

#[test]
fn info_opens_while_trimming_and_the_time_floats_over_the_picture() {
    screen_camera::SCREEN.set([1280, 720]);
    let ctx = context();
    let mut viewer = trimmer_on(&ctx, video());
    settle(&ctx, &mut viewer);
    let output = frame(&ctx, &mut viewer, vec![]);
    let time = text_rect(&output, &viewer.clock(viewer.position()));
    assert!(
        time.bottom() < viewer.view.bottom() && time.top() > viewer.view.top(),
        "the playhead's time is over the bottom of the picture: {time:?}"
    );

    click_named(&ctx, &mut viewer, "Info");
    assert!(viewer.is_info_open());
    assert!(viewer.is_trimming(), "the trim stays");
    settle(&ctx, &mut viewer);
    let nodes = named(&ctx, &mut viewer);
    assert!(
        nodes.iter().any(|(label, ..)| label == "Copy path"),
        "the card shows"
    );
    assert!(
        viewer.view.width() < 1000.0,
        "the picture narrows beside it"
    );
    let play = rect_of(&nodes, "Play");
    assert!(
        play.left() > viewer.view.left(),
        "the pill stays under the picture"
    );
}

#[test]
fn renaming_while_trimming_keeps_the_trim_but_waits_for_a_save() {
    let ctx = context();
    let folder = folder_of_videos("trim-rename", &["clip.mp4"]);
    let mut viewer = trimmer_on(&ctx, &folder.join("clip.mp4"));
    viewer.toggle_info();
    viewer.set_trim_start(seconds(0.5));
    viewer.set_trim_end(seconds(2.0));
    let kept = viewer.trim_range();

    viewer.rename_file(&ctx, "renamed");
    assert_eq!(
        viewer.current_path(),
        Some(folder.join("renamed.mp4").as_path())
    );
    assert!(viewer.is_trimming(), "the trimmer stays open");
    assert_eq!(viewer.trim_range(), kept, "with its handles");

    // A save still reading the file holds changes back, saying why.
    let _saving = viewer.pretend_saving(0.3);
    viewer.rename_file(&ctx, "again");
    assert_eq!(
        viewer.current_path(),
        Some(folder.join("renamed.mp4").as_path())
    );
    assert_eq!(
        viewer.info.as_ref().unwrap().problem.as_deref(),
        Some(info::TRIM_SAVING)
    );
    assert!(viewer.is_trimming());
}

/// Where the info card is, from its Copy path button's line down to the screen readers' view.
fn info_card_shows(ctx: &egui::Context, viewer: &mut Viewer) -> Option<Rect> {
    named(ctx, viewer)
        .into_iter()
        .find(|(label, ..)| label == "Copy path")
        .map(|(_, rect, _)| rect)
}

#[test]
fn info_shows_in_full_screen_clear_of_the_bars() {
    screen_camera::SCREEN.set([1280, 720]);
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    viewer.full_screen = true;
    viewer.toggle_info();
    settle(&ctx, &mut viewer);
    let copy = info_card_shows(&ctx, &mut viewer).expect("the card shows in full screen");
    assert!(
        copy.top() > bars::TITLE_HEIGHT,
        "its heading is below where the top bar slides in: {copy:?}"
    );
    assert!(
        viewer.view.right() < 1280.0 - info::CARD_WIDTH,
        "the picture narrows beside it"
    );
    // The bars hide as usual, and the card stays.
    viewer.last_pointer_move = Instant::now() - Duration::from_secs(10);
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(300.0, 360.0))],
    );
    settle(&ctx, &mut viewer);
    assert!(info_card_shows(&ctx, &mut viewer).is_some());
    viewer.full_screen = false;
}

#[test]
fn in_a_narrow_window_info_lies_over_the_picture_and_keeps_the_wheel_off_it() {
    screen_camera::SCREEN.set([640, 400]);
    let ctx = context();
    let mut viewer = opened(&ctx, video());
    settle(&ctx, &mut viewer);
    let whole_picture = viewer.view;
    viewer.toggle_info();
    settle(&ctx, &mut viewer);
    let copy = info_card_shows(&ctx, &mut viewer).expect("the card shows in a narrow window");
    assert_eq!(viewer.view, whole_picture, "the picture keeps its size");
    assert!(
        whole_picture.contains(copy.center()),
        "the card lies over it"
    );
    let card = ctx
        .read_response(egui::Id::unique("viewer_info_card"))
        .expect("the card is drawn")
        .rect;
    frame(
        &ctx,
        &mut viewer,
        vec![egui::Event::PointerMoved(pos2(100.0, 200.0))],
    );
    let nodes = named(&ctx, &mut viewer);
    assert!(
        rect_of(&nodes, "Play").top() > card.bottom(),
        "the pill shows below it: {card:?}"
    );
    if let Some((_, next, _)) = nodes.iter().find(|(label, ..)| label == "Next file") {
        assert!(next.right() < card.left(), "the next arrow is left of it");
    }
    assert!(
        card.contains(pos2(copy.center().x, copy.bottom() + 120.0)),
        "the wheel below goes on the card"
    );

    // The wheel over the card doesn't zoom the picture under it.
    let fit = viewer.zoom;
    let on_card = pos2(copy.center().x, copy.bottom() + 120.0);
    frame(&ctx, &mut viewer, wheel(on_card, 2.0));
    settle(&ctx, &mut viewer);
    assert_eq!(viewer.zoom, fit, "nothing zoomed");
    screen_camera::SCREEN.set([1280, 720]);
}
