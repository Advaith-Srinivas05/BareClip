//! Recording the screen or a window, with the PC's sound and the microphone on tracks of their
//! own. The bar floats over the picture like the microphone's, with a small live picture of what
//! will be recorded. While recording, BareClip's windows keep themselves out of the picture, a
//! small overlay stays on top of everything with Pause, Stop and the microphone, and keys pause
//! and stop from any program. The overlay starts near the screen's top right corner, can be
//! dragged by its grip and folded down to Pause and Stop.
//!
//! With a camera too, three files are recorded side by side: the screen with the PC's sound, the
//! camera with no sound, and the microphone on its own. All three are opened paused and set going
//! at the same moment, so they line up, and they land as three clips: the camera in a corner over
//! the screen, and the microphone on a sound row. The overlay can then show the camera too.

use std::sync::mpsc;

use bareclip_audio::{Microphone, SoundRecording};
use bareclip_capture::{
    AppSound, Offered, PictureSource, ScreenRecording, ScreenSettings, SoundFeed, Source,
};
use bareclip_core::placement::{CanvasCorner, Placement};
use bareclip_core::tracks::TrackTarget;
use bareclip_media::SoundLabel;

use super::camera_recording::{
    CameraPick, camera_choices, camera_settings, camera_state_text, draw_live_picture,
    microphone_choice,
};
use super::recording::{
    COLUMN_GAP, GROUP_GAP, MicSource, PICTURE_WIDTH, TIME_WIDTH, WIDE_BAR_WIDTH, choice_heading,
    folder_box, in_column, local_time, meter_fill, pause_button, picture_spot, place_bar,
    recording_name, shown_microphone, sound_switch, stop_button, time_label, unused_path,
};
use super::*;
use crate::{
    hotkeys::{self, RecordingKey, RecordingKeys},
    storage::{RecordingMarker, RecordingQuality, RecordingSize},
};

/// Each frame rate's button in the Options card.
const FRAME_RATE_WIDTH: f32 = 64.0;
/// The size and quality menus in the Options card.
const CHOICE_WIDTH: f32 = 196.0;
/// How often the small picture of what will be recorded is redrawn, in seconds.
const PREVIEW_EVERY: f64 = 0.2;
const PREVIEW_WIDTH: u32 = 336;
const COUNTDOWN_SECONDS: f64 = 3.0;
/// How far in from the screen's edge the overlay sits.
const OVERLAY_MARGIN: f32 = 24.0;
/// How far down from the screen's top the overlay starts, below where full-screen windows keep
/// their minimise, maximise and close buttons.
const OVERLAY_TOP: f32 = 72.0;
/// The room around the overlay's controls.
pub(super) const OVERLAY_PADDING: i8 = 8;
/// The room between the recording controls.
const CONTROL_SPACING: f32 = 4.0;
const CONTROL_SIZE: f32 = 32.0;
const GRIP_WIDTH: f32 = 16.0;
const FOLD_WIDTH: f32 = 24.0;
/// The room above the camera's picture in the overlay.
const CAMERA_GAP: f32 = 6.0;
/// Which corner the camera lands in over the screen.
const CAMERA_CORNER: CanvasCorner = CanvasCorner::BottomRight;

/// Starts capturing a screen or window, with or without the pointer.
pub(super) type OpenScreen = fn(Source, bool) -> Result<Box<dyn PictureSource>, String>;
/// Starts listening to the PC's sound.
pub(super) type OpenPcSound = fn() -> Result<MicSource, String>;
/// Starts listening to the sound of the program that owns a window.
pub(super) type OpenAppSound = fn(Source) -> Result<MicSource, String>;
/// Everything that can be recorded: screens first, then windows.
pub(super) type ListSources = fn() -> Vec<Offered>;

pub(super) fn open_real_screen(
    source: Source,
    pointer: bool,
) -> Result<Box<dyn PictureSource>, String> {
    bareclip_capture::ScreenCapture::start(source, pointer)
        .map(|capture| Box::new(capture) as Box<dyn PictureSource>)
}

pub(super) fn open_real_pc_sound() -> Result<MicSource, String> {
    let mut input = bareclip_audio::MicrophoneInput::open_pc_sound()?;
    Ok(MicSource {
        queue: input.take_queue(),
        rate: input.sample_rate(),
        level: input.level(),
        _keep: Box::new(input),
    })
}

pub(super) fn open_real_app_sound(source: Source) -> Result<MicSource, String> {
    let mut sound = AppSound::for_window(source)?;
    Ok(MicSource {
        queue: sound.take_queue(),
        rate: sound.sample_rate(),
        level: sound.level(),
        _keep: Box::new(sound),
    })
}

pub(super) fn list_real_sources() -> Vec<Offered> {
    let mut all = bareclip_capture::screens();
    all.extend(bareclip_capture::windows());
    all
}

pub(super) enum ScreenStage {
    Waiting,
    Counting(f64),
    Recording(Box<LiveScreen>),
}

/// The camera's file, recorded beside the screen's.
struct LiveCameraFile {
    recording: ScreenRecording,
    path: PathBuf,
    marker: Option<RecordingMarker>,
}

/// The microphone's file, recorded beside the screen's when there is a camera too.
struct LiveMicrophoneFile {
    recording: SoundRecording,
    path: PathBuf,
    marker: Option<RecordingMarker>,
    /// Whether it was ever on. A file that was silent the whole time is thrown away.
    heard: bool,
}

pub(super) struct LiveScreen {
    recording: ScreenRecording,
    path: PathBuf,
    marker: Option<RecordingMarker>,
    lands_at: Ticks,
    keys: RecordingKeys,
    pressed: mpsc::Receiver<RecordingKey>,
    /// The microphone's track in the file. There is one whenever a microphone could be opened,
    /// even if it starts silenced, so it can be turned on midway. With a camera, the microphone
    /// has a file of its own instead.
    microphone_track: Option<usize>,
    camera: Option<LiveCameraFile>,
    microphone_file: Option<LiveMicrophoneFile>,
    /// The overlay is folded down to Pause and Stop. Every recording starts unfolded.
    folded: bool,
    /// The overlay shows the camera's picture. Every recording starts without it.
    camera_shown: bool,
    /// Where the overlay first appears. It stays put from then on, until dragged.
    overlay_at: egui::Pos2,
}

impl LiveScreen {
    /// Whether the microphone is silenced, or `None` without a microphone.
    fn microphone_muted(&self) -> Option<bool> {
        if let Some(file) = &self.microphone_file {
            return Some(file.recording.is_muted());
        }
        self.microphone_track
            .map(|track| self.recording.is_muted(track))
    }

    fn set_paused(&self, paused: bool) {
        self.recording.set_paused(paused);
        if let Some(camera) = &self.camera {
            camera.recording.set_paused(paused);
        }
        if let Some(file) = &self.microphone_file {
            file.recording.set_paused(paused);
        }
    }

    fn set_microphone_muted(&mut self, muted: bool) {
        if let Some(file) = &mut self.microphone_file {
            file.recording.set_muted(muted);
            file.heard |= !muted;
        } else if let Some(track) = self.microphone_track {
            self.recording.set_muted(track, muted);
        }
    }
}

/// What the recording controls show.
#[derive(Clone, Copy)]
struct Shown {
    seconds: f64,
    paused: bool,
    microphone_muted: Option<bool>,
    folded: bool,
    /// Whether the camera's picture shows, or `None` without a camera.
    camera_shown: Option<bool>,
}

/// The overlay's size: the grip, the controls and the fold button, with room around them, and
/// the camera's picture below them when it shows. `camera` is `None` without a camera, and
/// otherwise whether its picture shows.
pub(super) fn overlay_size(folded: bool, camera: Option<bool>) -> egui::Vec2 {
    let mut controls = vec![GRIP_WIDTH, CONTROL_SIZE, CONTROL_SIZE];
    if !folded {
        controls.extend([TIME_WIDTH, CONTROL_SIZE]);
        if camera.is_some() {
            controls.push(CONTROL_SIZE);
        }
    }
    controls.push(FOLD_WIDTH);
    let gaps = CONTROL_SPACING * (controls.len() - 1) as f32;
    let padding = 2.0 * f32::from(OVERLAY_PADDING);
    let inner = controls.iter().sum::<f32>() + gaps;
    let picture = if !folded && camera == Some(true) {
        CAMERA_GAP + camera_picture_height(inner)
    } else {
        0.0
    };
    vec2(inner + padding, CONTROL_SIZE + picture + padding)
}

/// The camera's picture in the overlay: as wide as the controls, in the usual 16:9.
fn camera_picture_height(width: f32) -> f32 {
    (width * 9.0 / 16.0).round()
}

/// Where the overlay starts: near the top right corner of a screen this size.
pub(super) fn overlay_start(screen: egui::Vec2, overlay: egui::Vec2) -> egui::Pos2 {
    pos2(
        (screen.x - overlay.x - OVERLAY_MARGIN).max(0.0),
        OVERLAY_TOP,
    )
}

/// The screen recording bar, while it's open.
pub(super) struct ScreenBar {
    offered: Vec<Offered>,
    chosen: Option<Source>,
    capture: Result<Box<dyn PictureSource>, String>,
    preview: Option<TextureHandle>,
    preview_count: u64,
    preview_at: f64,
    pc_sound: Option<Result<MicSource, String>>,
    /// Whether `pc_sound` is only the chosen window's program.
    app_sound: bool,
    /// Why only the window's program couldn't be listened to, when all PC sound is instead.
    app_sound_problem: Option<String>,
    microphones: Vec<Microphone>,
    /// The microphone picked for this recording, starting on Settings' default. `None` is
    /// Windows' default.
    chosen_microphone: Option<String>,
    microphone: Option<Result<MicSource, String>>,
    pc_level: f32,
    microphone_level: f32,
    /// The camera recorded beside the screen, for Screen and camera.
    pub(super) camera: Option<CameraPick>,
    /// A folder picked with Browse for this recording only.
    pub(super) folder: Option<PathBuf>,
    /// The quality and size picked for this recording, starting on Settings' defaults.
    quality: RecordingQuality,
    size: RecordingSize,
    pub(super) stage: ScreenStage,
}

/// The PC's sound as a recording bar listens to it: all of it, or only a window's program.
struct ScreenSound {
    source: Option<Result<MicSource, String>>,
    app: bool,
    problem: Option<String>,
}

impl ScreenBar {
    pub(super) fn is_recording(&self) -> bool {
        !matches!(self.stage, ScreenStage::Waiting)
    }

    fn chosen_name(&self) -> String {
        self.chosen
            .and_then(|source| self.offered.iter().find(|offered| offered.source == source))
            .map_or_else(
                || "Nothing to record".to_owned(),
                |offered| offered.name.clone(),
            )
    }

    /// The size of what will be recorded, as it is now.
    fn source_size(&self) -> (u32, u32) {
        match &self.capture {
            Ok(capture) => capture.size(),
            Err(_) => self
                .chosen
                .and_then(|source| self.offered.iter().find(|offered| offered.source == source))
                .map_or((1920, 1080), |offered| offered.size),
        }
    }

    /// How the screen will be recorded at this frame rate, with the size and quality picked.
    pub(super) fn screen_settings(&self, frame_rate: u32) -> ScreenSettings {
        ScreenSettings {
            frame_rate,
            size: self.size.video_size(self.source_size()),
            quality: self.quality.into(),
        }
    }

    fn is_window(&self) -> bool {
        matches!(self.chosen, Some(Source::Window(_)))
    }

    fn set_sound(&mut self, sound: ScreenSound) {
        // The old one stops being listened to as it's replaced.
        self.pc_sound = sound.source;
        self.app_sound = sound.app;
        self.app_sound_problem = sound.problem;
        self.pc_level = 0.0;
    }

    /// Whether everything to be recorded is ready.
    fn ready(&self) -> bool {
        self.capture.is_ok() && self.camera.as_ref().is_none_or(CameraPick::is_ready)
    }
}

/// A small copy of a picture for the bar, about `width` pixels across.
pub(super) fn shrunk(picture: &bareclip_capture::Picture, width: u32) -> egui::ColorImage {
    let scale = (picture.width as f32 / width as f32).max(1.0);
    let out_width = (picture.width as f32 / scale).round().max(1.0) as usize;
    let out_height = (picture.height as f32 / scale).round().max(1.0) as usize;
    let mut rgba = Vec::with_capacity(out_width * out_height * 4);
    for row in 0..out_height {
        let from_row = ((row as f32 * scale) as usize).min(picture.height as usize - 1);
        for column in 0..out_width {
            let from_column = ((column as f32 * scale) as usize).min(picture.width as usize - 1);
            let at = (from_row * picture.width as usize + from_column) * 4;
            let [b, g, r, _] = [
                picture.bgra[at],
                picture.bgra[at + 1],
                picture.bgra[at + 2],
                picture.bgra[at + 3],
            ];
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([out_width, out_height], &rgba)
}

/// Whether a recording finished. If so, its marker goes, and otherwise what went wrong is noted,
/// and the marker stays so it's offered on the next start.
fn finished<T>(
    stopped: Result<T, bareclip_media::MediaError>,
    path: &Path,
    marker: Option<RecordingMarker>,
    problems: &mut Vec<String>,
) -> bool {
    match stopped {
        Ok(_) => {
            if let Some(marker) = marker {
                marker.finish();
            }
            true
        }
        Err(error) => {
            problems.push(format!(
                "{} could not be finished, but what was recorded is in it. {error}",
                path.display()
            ));
            false
        }
    }
}

/// Where the camera's box sits over a picture of the screen, the way it will land: Picture in
/// picture's size, in its corner. Shares of the picture, so it fits a preview of any size.
pub(super) fn camera_box_share(camera: (u32, u32), screen: (u32, u32)) -> egui::Rect {
    let placed = Placement::default().in_corner(CAMERA_CORNER, camera, screen);
    let rect = placed.rect(camera, screen);
    let (width, height) = (f64::from(screen.0.max(1)), f64::from(screen.1.max(1)));
    egui::Rect::from_min_size(
        pos2(
            (f64::from(rect.x) / width) as f32,
            (f64::from(rect.y) / height) as f32,
        ),
        vec2(
            (f64::from(rect.width) / width) as f32,
            (f64::from(rect.height) / height) as f32,
        ),
    )
}

impl Editor {
    /// Listens to the PC's sound for a recording of `source`, if it's switched on: only the
    /// window's program when Settings say so, falling back to all of it if that can't be done.
    fn open_screen_sound(&self, source: Option<Source>) -> ScreenSound {
        if !self.settings.screen_pc_sound {
            return ScreenSound {
                source: None,
                app: false,
                problem: None,
            };
        }
        if self.settings.screen_app_sound_only
            && let Some(window @ Source::Window(_)) = source
        {
            match (self.open_app_sound)(window) {
                Ok(sound) => {
                    return ScreenSound {
                        source: Some(Ok(sound)),
                        app: true,
                        problem: None,
                    };
                }
                Err(problem) => {
                    return ScreenSound {
                        source: Some((self.open_pc_sound)()),
                        app: false,
                        problem: Some(format!("{problem} {}", text::ALL_PC_SOUND_INSTEAD)),
                    };
                }
            }
        }
        ScreenSound {
            source: Some((self.open_pc_sound)()),
            app: false,
            problem: None,
        }
    }

    /// Listens to the PC's sound again, after the source or a sound choice changed.
    fn reopen_screen_sound(&mut self) {
        let Some(chosen) = self.screen_bar.as_ref().map(|bar| bar.chosen) else {
            return;
        };
        // The old sound is let go first, so a program's sound is never listened to twice.
        if let Some(bar) = &mut self.screen_bar {
            bar.pc_sound = None;
        }
        let sound = self.open_screen_sound(chosen);
        if let Some(bar) = &mut self.screen_bar {
            bar.set_sound(sound);
        }
    }

    /// Opens the screen recording bar, with the camera's choices too for Screen and camera.
    pub(super) fn open_screen_bar(&mut self, with_camera: bool) {
        let offered = (self.list_sources)();
        let chosen = offered
            .iter()
            .find(|offered| offered.is_main)
            .or_else(|| offered.first())
            .map(|offered| offered.source);
        let capture = match chosen {
            Some(source) => (self.open_screen)(source, self.settings.screen_pointer),
            None => Err("There is nothing to record.".to_owned()),
        };
        let sound = self.open_screen_sound(chosen);
        let microphones = (self.list_microphones)();
        let chosen_microphone = self.default_microphone(&microphones);
        let microphone = self
            .settings
            .screen_microphone
            .then(|| (self.open_microphone)(chosen_microphone.as_deref()));
        let camera = with_camera.then(|| CameraPick::new((self.list_cameras)()));
        self.screen_bar = Some(ScreenBar {
            offered,
            chosen,
            capture,
            preview: None,
            preview_count: 0,
            preview_at: 0.0,
            pc_sound: sound.source,
            app_sound: sound.app,
            app_sound_problem: sound.problem,
            microphones,
            chosen_microphone,
            microphone,
            pc_level: 0.0,
            microphone_level: 0.0,
            camera,
            folder: None,
            quality: self.settings.recording_quality,
            size: self.settings.screen_size,
            stage: ScreenStage::Waiting,
        });
        if with_camera {
            self.open_chosen_camera();
        }
    }

    fn choose_source(&mut self, source: Source) {
        let pointer = self.settings.screen_pointer;
        let open = self.open_screen;
        if let Some(bar) = &mut self.screen_bar {
            bar.chosen = Some(source);
            // The old capture stops as it's replaced.
            bar.capture = open(source, pointer);
            bar.preview = None;
            bar.preview_count = 0;
        }
        // Another window can belong to another program.
        if self.settings.screen_pc_sound && self.settings.screen_app_sound_only {
            self.reopen_screen_sound();
        }
    }

    /// Picks a microphone for this recording only, and listens to it if it's switched on.
    /// Settings' default stays as it is.
    fn choose_screen_microphone(&mut self, name: String) {
        let open = self.open_microphone;
        if let Some(bar) = &mut self.screen_bar {
            if bar.microphone.is_some() {
                // The old one stops being listened to as it's replaced.
                bar.microphone = Some(open(Some(&name)));
                bar.microphone_level = 0.0;
            }
            bar.chosen_microphone = Some(name);
        }
    }

    fn start_screen_recording(&mut self, ctx: &egui::Context) {
        let default_folder = self.recordings_folder();
        let lands_at = self.position();
        let open_microphone = self.open_microphone;
        let microphone_name = self
            .screen_bar
            .as_ref()
            .and_then(|bar| bar.chosen_microphone.clone());
        let microphone_on = self.settings.screen_microphone;
        let monitor = ctx
            .input(|input| input.viewport().monitor_size)
            .unwrap_or(vec2(1920.0, 1080.0));
        let frame_rate = self.settings.screen_frame_rate;
        let shortcuts = [
            (RecordingKey::Stop, self.settings.stop_key),
            (RecordingKey::Pause, self.settings.pause_key),
            (RecordingKey::Microphone, self.settings.microphone_key),
        ];
        let Some(bar) = &mut self.screen_bar else {
            return;
        };
        let settings = bar.screen_settings(frame_rate);
        let quality = bar.quality;
        let pc_sound_name = if bar.app_sound {
            "App sound"
        } else {
            "PC sound"
        };
        let with_camera = bar.camera.is_some();
        let overlay_at = overlay_start(monitor, overlay_size(false, with_camera.then_some(false)));
        let camera = match &mut bar.camera {
            Some(pick) => match pick.take_running() {
                Some(running) => Some(running),
                None => {
                    bar.stage = ScreenStage::Waiting;
                    return;
                }
            },
            None => None,
        };
        let capture = std::mem::replace(&mut bar.capture, Err("Recording".to_owned()));
        let Ok(capture) = capture else {
            bar.capture = capture;
            bar.stage = ScreenStage::Waiting;
            return;
        };
        let folder = bar.folder.clone().unwrap_or(default_folder);
        // The microphone is always listened to, silenced when switched off, so it can be turned
        // on midway. A track can't be added once the file has begun.
        if bar.microphone.is_none() {
            bar.microphone = Some(open_microphone(microphone_name.as_deref()));
        }
        let mut sounds = Vec::new();
        let mut microphone_track = None;
        if let Some(Ok(source)) = &mut bar.pc_sound
            && let Some(queue) = source.queue.take()
        {
            sounds.push(SoundFeed {
                queue,
                rate: source.rate,
                name: pc_sound_name.to_owned(),
                muted: false,
            });
        }
        // With a camera the microphone gets a file of its own, otherwise a track in the screen's.
        let mut microphone_queue = None;
        if let Some(Ok(source)) = &mut bar.microphone
            && let Some(queue) = source.queue.take()
        {
            if with_camera {
                microphone_queue = Some((queue, source.rate));
            } else {
                microphone_track = Some(sounds.len());
                sounds.push(SoundFeed {
                    queue,
                    rate: source.rate,
                    name: "Microphone".to_owned(),
                    muted: !microphone_on,
                });
            }
        }
        if let Err(error) = std::fs::create_dir_all(&folder) {
            self.notice = Some(format!(
                "The recording could not start, because {} could not be made. {error}",
                folder.display()
            ));
            self.screen_bar = None;
            return;
        }
        let when = local_time();
        let path = unused_path(&folder, &recording_name("Screen", when, "mp4"));
        // The keys are taken first, so the file starts only once they are ready.
        let (sender, pressed) = mpsc::channel();
        let wake = ctx.clone();
        let keys = RecordingKeys::listen(shortcuts, sender, move || wake.request_repaint());
        let marker_for = |editor: &Self, path: &Path| {
            editor
                .app_folder
                .as_ref()
                .and_then(|app| RecordingMarker::start(app, path).ok())
        };
        // With a camera, everything opens paused and goes on together, so the files line up
        // however long each encoder takes to open.
        let start = if with_camera {
            ScreenRecording::start_paused
        } else {
            ScreenRecording::start
        };
        let recording = match start(&path, capture, sounds, settings) {
            Ok(recording) => recording,
            Err(error) => {
                self.notice = Some(format!("The recording could not start. {error}"));
                self.screen_bar = None;
                return;
            }
        };
        let marker = marker_for(self, &path);
        let mut camera_file = None;
        if let Some((source, mode)) = camera {
            let camera_path = unused_path(&folder, &recording_name("Camera", when, "mp4"));
            match ScreenRecording::start_paused(
                &camera_path,
                Box::new(source),
                Vec::new(),
                camera_settings(mode, quality),
            ) {
                Ok(camera_recording) => {
                    camera_file = Some(LiveCameraFile {
                        recording: camera_recording,
                        marker: marker_for(self, &camera_path),
                        path: camera_path,
                    });
                }
                Err(error) => {
                    self.notice = Some(format!("The recording could not start. {error}"));
                    let _ = recording.stop();
                    if let Some(marker) = marker {
                        marker.finish();
                    }
                    let _ = std::fs::remove_file(&path);
                    self.screen_bar = None;
                    return;
                }
            }
        }
        let mut microphone_file = None;
        if let Some((queue, rate)) = microphone_queue {
            let microphone_path = unused_path(&folder, &recording_name("Microphone", when, "m4a"));
            let label = SoundLabel {
                title: Some("Microphone".to_owned()),
                language: None,
            };
            match SoundRecording::start_paused(&microphone_path, queue, rate, label) {
                Ok(sound) => {
                    sound.set_muted(!microphone_on);
                    microphone_file = Some(LiveMicrophoneFile {
                        recording: sound,
                        marker: marker_for(self, &microphone_path),
                        path: microphone_path,
                        heard: microphone_on,
                    });
                }
                Err(error) => {
                    self.notice = Some(format!(
                        "The microphone could not be recorded, so the screen and camera are recorded without it. {error}"
                    ));
                }
            }
        }
        // Everything goes on at the same moment.
        if with_camera {
            recording.set_paused(false);
            if let Some(camera) = &camera_file {
                camera.recording.set_paused(false);
            }
            if let Some(file) = &microphone_file {
                file.recording.set_paused(false);
            }
        }
        hotkeys::hide_from_captures(true);
        if !keys.taken {
            self.notice = Some(format!(
                "Another program uses one of {}, so use the recording bar instead.",
                shortcuts
                    .iter()
                    .map(|(_, shortcut)| shortcut.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(bar) = &mut self.screen_bar {
            bar.stage = ScreenStage::Recording(Box::new(LiveScreen {
                recording,
                path,
                marker,
                lands_at,
                keys,
                pressed,
                microphone_track,
                camera: camera_file,
                microphone_file,
                folded: false,
                camera_shown: false,
                overlay_at,
            }));
        }
    }

    /// Finishes the files and adds them to the project.
    pub(super) fn stop_screen_recording(&mut self, ctx: &egui::Context) {
        let Some(bar) = self.screen_bar.take() else {
            return;
        };
        let ScreenStage::Recording(live) = bar.stage else {
            return;
        };
        // Everything stops taking pictures and sound at the same moment. Finishing a file can take
        // a while, and the others would go on recording meanwhile.
        live.set_paused(true);
        let LiveScreen {
            recording,
            path,
            marker,
            lands_at,
            keys,
            camera,
            microphone_file,
            ..
        } = *live;
        drop(keys);
        hotkeys::hide_from_captures(false);
        let mut problems = Vec::new();
        let screen_done = finished(recording.stop(), &path, marker, &mut problems);
        let camera_path = camera.and_then(|camera| {
            let stopped = camera.recording.stop();
            finished(stopped, &camera.path, camera.marker, &mut problems).then_some(camera.path)
        });
        let microphone_path = microphone_file.and_then(|file| {
            let stopped = file.recording.stop();
            if !file.heard {
                // Silent all along, so there's nothing to add.
                if let Some(marker) = file.marker {
                    marker.finish();
                }
                let _ = std::fs::remove_file(&file.path);
                return None;
            }
            finished(stopped, &file.path, file.marker, &mut problems).then_some(file.path)
        });
        if !problems.is_empty() {
            self.notice = Some(problems.join(" "));
        }
        if !screen_done {
            return;
        }
        match camera_path {
            Some(camera_path) => self.add_screen_and_camera(
                ctx,
                &path,
                &camera_path,
                microphone_path.as_deref(),
                lands_at,
            ),
            None => self.add_recording(ctx, &path, lands_at),
        }
    }

    /// Puts a screen, camera and microphone recorded together in My media and on new rows at
    /// `at`, moving nothing, as one step to undo: the screen at full size, the camera over it in
    /// its corner at Picture in picture's size, and the microphone on a sound row.
    fn add_screen_and_camera(
        &mut self,
        ctx: &egui::Context,
        screen: &Path,
        camera: &Path,
        microphone: Option<&Path>,
        at: Ticks,
    ) {
        let mut loaded = Vec::new();
        for path in [Some(screen), Some(camera), microphone]
            .into_iter()
            .flatten()
        {
            match self.load(ctx, path) {
                Ok(mut entry) => {
                    entry.recorded = true;
                    let id = entry.id;
                    self.media.push(entry);
                    loaded.push(self.full_range(id).map(|range| (id, range)));
                }
                Err(error) => {
                    let name = path
                        .file_name()
                        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                    self.notice = Some(format!("{name} could not be opened. {error}"));
                    loaded.push(None);
                }
            }
        }
        let recorded = |found: Option<&Option<(MediaId, TimeRange)>>| {
            found
                .copied()
                .flatten()
                .filter(|(_, range)| range.duration().0 > 0)
        };
        let (screen, camera, microphone) = (
            recorded(loaded.first()),
            recorded(loaded.get(1)),
            recorded(loaded.get(2)),
        );
        let Some((screen_id, screen_range)) = screen else {
            self.notice
                .get_or_insert_with(|| "Nothing was recorded.".to_owned());
            return;
        };
        self.record();
        let screen_clip =
            self.project
                .tracks
                .place(screen_id, screen_range, at, TrackTarget::New(0));
        // The screen sets an empty project's size, being the main picture.
        self.settle_canvas();
        if let Some((camera_id, camera_range)) = camera {
            let clip = self
                .project
                .tracks
                .place(camera_id, camera_range, at, TrackTarget::New(0));
            let canvas = self.project.canvas.size();
            if let Some(size) = self.video_size(camera_id) {
                let placed = Placement::default().in_corner(CAMERA_CORNER, size, canvas);
                self.project.tracks.set_placement(clip, placed);
            }
        }
        if let Some((microphone_id, microphone_range)) = microphone {
            let tracks = &mut self.project.tracks;
            let row = TrackTarget::New(tracks.picture_rows());
            tracks.place_sound(microphone_id, microphone_range, at, row);
        }
        self.selected = Some(screen_clip);
        self.settle_canvas();
        self.after_clip_added();
    }

    /// Does what a recording key pressed in any program asks. Gives whether it stopped the
    /// recording.
    pub(super) fn recording_key_pressed(&mut self, ctx: &egui::Context, key: RecordingKey) -> bool {
        match key {
            RecordingKey::Stop => {
                self.stop_screen_recording(ctx);
                return true;
            }
            RecordingKey::Pause => self.with_live_screen(|live| {
                let paused = live.recording.is_paused();
                live.set_paused(!paused);
            }),
            // Nothing to turn on without a microphone.
            RecordingKey::Microphone => self.with_live_screen(|live| {
                if let Some(muted) = live.microphone_muted() {
                    live.set_microphone_muted(!muted);
                }
            }),
        }
        false
    }

    fn with_live_screen(&mut self, change: impl FnOnce(&mut LiveScreen)) {
        if let Some(ScreenBar {
            stage: ScreenStage::Recording(live),
            ..
        }) = &mut self.screen_bar
        {
            change(live);
        }
    }

    /// Keeps the small pictures and the meters up to date.
    fn refresh_screen_bar(&mut self, ctx: &egui::Context, now: f64) {
        let Some(bar) = &mut self.screen_bar else {
            return;
        };
        for (source, level) in [
            (&bar.pc_sound, &mut bar.pc_level),
            (&bar.microphone, &mut bar.microphone_level),
        ] {
            if let Some(Ok(source)) = source {
                *level = (*level * 0.85).max(meter_fill(source.level.take()));
            }
        }
        // The camera's picture keeps coming while recording, for the overlay.
        if let Some(pick) = &mut bar.camera {
            pick.refresh(ctx, now);
        }
        if now - bar.preview_at < PREVIEW_EVERY {
            return;
        }
        bar.preview_at = now;
        if let Ok(capture) = &bar.capture
            && let Some((picture, count)) = capture.latest()
            && count != bar.preview_count
        {
            bar.preview_count = count;
            let image = shrunk(&picture, PREVIEW_WIDTH);
            match &mut bar.preview {
                Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                None => {
                    bar.preview = Some(ctx.load_texture(
                        "screen-preview",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
            }
        }
    }

    /// The screen recording bar, and the overlay while recording.
    pub(super) fn screen_bar_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        if self.screen_bar.is_none() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(33));
        let now = ctx.input(|input| input.time);
        self.refresh_screen_bar(ctx, now);
        if let Some(ScreenBar {
            stage: ScreenStage::Counting(until),
            ..
        }) = &self.screen_bar
            && now >= *until
        {
            self.start_screen_recording(ctx);
        }
        // Keys pressed in another program.
        let mut keyed = Vec::new();
        if let Some(ScreenBar {
            stage: ScreenStage::Recording(live),
            ..
        }) = &self.screen_bar
        {
            while let Ok(key) = live.pressed.try_recv() {
                keyed.push(key);
            }
        }
        for key in keyed {
            if self.recording_key_pressed(ctx, key) {
                return;
            }
        }
        let Some(bar) = &self.screen_bar else {
            return;
        };
        enum Asked {
            Nothing,
            Record,
            Cancel,
            Stop,
            Pause(bool),
            Source(Source),
            PcSound(bool),
            AppSoundOnly(bool),
            Microphone(bool),
            ChooseMicrophone(String),
            Camera(super::camera_recording::CameraAsk),
            FrameRate(u32),
            Size(RecordingSize),
            Quality(RecordingQuality),
            Pointer(bool),
            Countdown(bool),
            MicrophoneMuted(bool),
            Folded(bool),
            CameraShown(bool),
            Browse,
        }
        let mut asked = Asked::Nothing;
        let recording = match &bar.stage {
            ScreenStage::Recording(live) => Some((
                Shown {
                    seconds: live.recording.seconds(),
                    paused: live.recording.is_paused(),
                    microphone_muted: live.microphone_muted(),
                    folded: live.folded,
                    camera_shown: live.camera.as_ref().map(|_| live.camera_shown),
                },
                live.overlay_at,
            )),
            _ => None,
        };
        let counting = match bar.stage {
            ScreenStage::Counting(until) => Some((until - now).ceil().max(1.0) as u32),
            _ => None,
        };
        let stage = self.stage_rect;
        let settings = self.settings.clone();
        let shown_mic = shown_microphone(bar.chosen_microphone.as_ref(), &bar.microphones);
        let folder = bar
            .folder
            .clone()
            .unwrap_or_else(|| self.recordings_folder());
        let camera_picture = bar.camera.as_ref().and_then(|pick| pick.preview.clone());
        let camera_box = bar.camera.as_ref().and_then(|pick| match pick.state() {
            bareclip_capture::CameraState::Running { mode, .. } => {
                let screen = bar
                    .chosen
                    .and_then(|source| bar.offered.iter().find(|offered| offered.source == source))
                    .map_or((1920, 1080), |offered| offered.size);
                Some(camera_box_share((mode.width, mode.height), screen))
            }
            _ => None,
        });
        let source_size = bar.source_size();
        let size_shown = bar.size.for_source(source_size);
        let keys_line = text::recording_keys_line(
            settings.stop_key,
            settings.pause_key,
            settings.microphone_key,
        );
        let title = if bar.camera.is_some() {
            "Record screen and camera"
        } else {
            "Record screen"
        };
        let area = egui::Area::new(egui::Id::unique("screen_bar")).order(egui::Order::Foreground);
        place_bar(area, ctx, stage, recording.is_some())
            .constrain(true)
            .show(ctx, |ui| {
                if let Some((shown, _)) = recording {
                    dialog_frame(ctx)
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                                if let Some(choice) = recording_controls(ui, palette, shown) {
                                    asked = choice;
                                }
                            });
                            if shown.camera_shown == Some(true)
                                && let Some(picture) = &camera_picture
                            {
                                camera_window(ui, palette, picture);
                            }
                        });
                    return;
                }
                dialog_frame(ctx).show(ui, |ui| {
                    ui.set_width(WIDE_BAR_WIDTH);
                    components::dialog_title(ui, &icons::MONITOR, palette.accent, title);
                    ui.add_space(12.0);
                    if let Some(left) = counting {
                        ui.vertical_centered(|ui| {
                            ui.colored_label(palette.text_muted, "Recording starts in");
                            ui.label(
                                egui::RichText::new(left.to_string())
                                    .font(fonts::strong(40.0))
                                    .color(palette.text),
                            );
                        });
                        ui.add_space(12.0);
                        components::dialog_buttons(ui, |ui| {
                            if ui.add(dialog_button(palette, "Cancel")).clicked() {
                                asked = Asked::Cancel;
                            }
                        });
                        return;
                    }
                    let width = ui.available_width();
                    let right = width - PICTURE_WIDTH - COLUMN_GAP;
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = COLUMN_GAP;
                        // What will be recorded, as it looks now, with the camera where it will
                        // land.
                        let spot = picture_spot(ui, palette);
                        match (&bar.capture, &bar.preview) {
                            (Ok(_), Some(texture)) => {
                                let size = texture.size_vec2();
                                let scale = (spot.width() / size.x).min(spot.height() / size.y);
                                let shown =
                                    egui::Rect::from_center_size(spot.center(), size * scale);
                                ui.painter().image(
                                    texture.id(),
                                    shown,
                                    egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                    Color32::WHITE,
                                );
                                if let (Some(share), Some(picture)) = (camera_box, &camera_picture)
                                {
                                    let inside = egui::Rect::from_min_size(
                                        shown.min + share.min.to_vec2() * shown.size(),
                                        share.size() * shown.size(),
                                    );
                                    ui.painter().rect_filled(inside, 0.0, palette.stage);
                                    draw_live_picture(ui.painter(), inside, picture);
                                }
                            }
                            (Err(problem), _) => {
                                ui.painter().text(
                                    spot.center(),
                                    egui::Align2::CENTER_CENTER,
                                    problem,
                                    egui::FontId::proportional(12.0),
                                    palette.danger,
                                );
                            }
                            _ => {}
                        }
                        if let Some(pick) = &bar.camera {
                            // Until the camera runs, the reason it doesn't goes over the picture.
                            camera_state_text(ui, palette, spot, &pick.state());
                        }
                        // What and where, beside the picture.
                        in_column(ui, right, |ui| {
                            choice_heading(ui, palette, "Record");
                            let shown = bar.chosen_name();
                            let name = format!("Record: {shown}");
                            let button =
                                ui.add(components::Dropdown::new(palette, &name, &shown, right));
                            let menu_width = components::menu_width(&button);
                            egui::Popup::menu(&button).show(|ui| {
                                egui::ScrollArea::vertical()
                                    .max_height(320.0)
                                    .show(ui, |ui| {
                                        for offered in &bar.offered {
                                            let item = MenuItem::choice(
                                                palette,
                                                &offered.name,
                                                bar.chosen == Some(offered.source),
                                            )
                                            .width(menu_width);
                                            if ui.add(item).clicked() {
                                                asked = Asked::Source(offered.source);
                                            }
                                        }
                                    });
                            });
                            if let Some(pick) = &bar.camera {
                                ui.add_space(10.0);
                                choice_heading(ui, palette, "Camera");
                                if let Some(choice) = camera_choices(ui, palette, pick, right) {
                                    asked = Asked::Camera(choice);
                                }
                            }
                            ui.add_space(10.0);
                            choice_heading(ui, palette, "Save in");
                            if folder_box(ui, palette, &folder, right) {
                                asked = Asked::Browse;
                            }
                        });
                    });
                    ui.add_space(GROUP_GAP);
                    // Sound and Options side by side, so the window stays short enough for a
                    // laptop's screen.
                    let column = (width - COLUMN_GAP) / 2.0;
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = COLUMN_GAP;
                        ui.allocate_ui_with_layout(
                            vec2(column, 0.0),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.set_width(column);
                                components::settings_group(ui, palette, "Sound", |ui| {
                                    if sound_switch(
                                        ui,
                                        palette,
                                        ("PC sound", settings.screen_pc_sound),
                                        bar.pc_sound.as_ref(),
                                        bar.pc_level,
                                    ) {
                                        asked = Asked::PcSound(!settings.screen_pc_sound);
                                    }
                                    // Recording a window can take only its program's sound.
                                    if bar.is_window() && settings.screen_pc_sound {
                                        let only = settings.screen_app_sound_only;
                                        let row = SwitchRow::new(
                                            palette,
                                            text::ONLY_THIS_APPS_SOUND,
                                            only,
                                        )
                                        .compact();
                                        if ui.add(row).clicked() {
                                            asked = Asked::AppSoundOnly(!only);
                                        }
                                        if let Some(problem) = &bar.app_sound_problem {
                                            components::settings_note(ui, palette, problem, None);
                                        }
                                    }
                                    components::settings_divider(ui, palette);
                                    if sound_switch(
                                        ui,
                                        palette,
                                        ("Microphone", settings.screen_microphone),
                                        bar.microphone.as_ref(),
                                        bar.microphone_level,
                                    ) {
                                        asked = Asked::Microphone(!settings.screen_microphone);
                                    }
                                    // Shown even while it's off, so it can be picked before switching it on.
                                    components::settings_wide_control(ui, |ui, width| {
                                        if let Some(name) = microphone_choice(
                                            ui,
                                            palette,
                                            &bar.microphones,
                                            &shown_mic,
                                            width,
                                        ) {
                                            asked = Asked::ChooseMicrophone(name);
                                        }
                                    });
                                });
                            },
                        );
                        ui.allocate_ui_with_layout(
                            vec2(column, 0.0),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.set_width(column);
                                components::settings_group(ui, palette, "Options", |ui| {
                                    let pointer = settings.screen_pointer;
                                    let row =
                                        SwitchRow::new(palette, "Show the mouse pointer", pointer)
                                            .compact();
                                    if ui.add(row).clicked() {
                                        asked = Asked::Pointer(!pointer);
                                    }
                                    components::settings_divider(ui, palette);
                                    let row = SwitchRow::new(
                                        palette,
                                        "Count down from 3",
                                        settings.countdown,
                                    )
                                    .compact();
                                    if ui.add(row).clicked() {
                                        asked = Asked::Countdown(!settings.countdown);
                                    }
                                    components::settings_divider(ui, palette);
                                    components::compact_settings_row(
                                        ui,
                                        palette,
                                        "Frame rate",
                                        FRAME_RATE_WIDTH * 2.0 + CONTROL_SPACING,
                                        |ui| {
                                            ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                                            for rate in [30, 60] {
                                                let chosen = settings.screen_frame_rate == rate;
                                                let label = format!("{rate} fps");
                                                let button =
                                                    ActionButton::new(palette, Kind::Ghost, &label)
                                                        .small()
                                                        .width(FRAME_RATE_WIDTH)
                                                        .selected(chosen);
                                                if ui.add(button).clicked() && !chosen {
                                                    asked = Asked::FrameRate(rate);
                                                }
                                            }
                                        },
                                    );
                                    components::settings_divider(ui, palette);
                                    components::compact_settings_row(
                                        ui,
                                        palette,
                                        "Size",
                                        CHOICE_WIDTH,
                                        |ui| {
                                            if let Some(size) = size_menu(
                                                ui,
                                                palette,
                                                (size_shown, source_size),
                                                CHOICE_WIDTH,
                                            ) {
                                                asked = Asked::Size(size);
                                            }
                                        },
                                    );
                                    components::settings_divider(ui, palette);
                                    components::compact_settings_row(
                                        ui,
                                        palette,
                                        "Quality",
                                        CHOICE_WIDTH,
                                        |ui| {
                                            if let Some(quality) =
                                                quality_menu(ui, palette, bar.quality, CHOICE_WIDTH)
                                            {
                                                asked = Asked::Quality(quality);
                                            }
                                        },
                                    );
                                });
                            },
                        );
                    });
                    ui.add_space(16.0);
                    components::dialog_buttons(ui, |ui| {
                        let record = ActionButton::new(palette, Kind::Primary, "Start recording")
                            .icon(icons::VIDEO)
                            .width(DIALOG_BUTTON_WIDTH * 1.5);
                        // The keys that stop and pause it, for whoever wonders.
                        if ui
                            .add_enabled(bar.ready(), record)
                            .on_hover_text(&keys_line)
                            .clicked()
                        {
                            asked = Asked::Record;
                        }
                        if ui.add(dialog_button(palette, "Cancel")).clicked() {
                            asked = Asked::Cancel;
                        }
                    });
                });
            });
        if let Some((shown, at)) = recording
            && let Some(choice) = recording_overlay(ctx, palette, shown, at, camera_picture.clone())
        {
            asked = choice;
        }
        if recording.is_some() {
            // The overlay is a window of its own, made after recording began.
            hotkeys::hide_from_captures(true);
        }
        if recording.is_none()
            && matches!(asked, Asked::Nothing)
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            asked = Asked::Cancel;
        }
        let save = |editor: &mut Self| editor.save_settings();
        match asked {
            Asked::Nothing => {}
            Asked::Record => {
                let wait = if self.settings.countdown {
                    COUNTDOWN_SECONDS
                } else {
                    0.0
                };
                if let Some(bar) = &mut self.screen_bar {
                    bar.stage = ScreenStage::Counting(now + wait);
                }
                if wait == 0.0 {
                    self.start_screen_recording(ctx);
                }
            }
            Asked::Cancel => self.screen_bar = None,
            Asked::Stop => self.stop_screen_recording(ctx),
            Asked::Pause(paused) => self.with_live_screen(|live| live.set_paused(paused)),
            Asked::Source(source) => self.choose_source(source),
            Asked::PcSound(on) => {
                self.settings.screen_pc_sound = on;
                self.reopen_screen_sound();
                save(self);
            }
            Asked::AppSoundOnly(on) => {
                self.settings.screen_app_sound_only = on;
                self.reopen_screen_sound();
                save(self);
            }
            Asked::Microphone(on) => {
                self.settings.screen_microphone = on;
                let open = self.open_microphone;
                if let Some(bar) = &mut self.screen_bar {
                    bar.microphone = on.then(|| open(bar.chosen_microphone.as_deref()));
                }
                save(self);
            }
            Asked::ChooseMicrophone(name) => self.choose_screen_microphone(name),
            Asked::Camera(choice) => self.camera_asked(choice),
            Asked::FrameRate(rate) => {
                self.settings.screen_frame_rate = rate;
                save(self);
            }
            // Both last for this recording only. Settings keep the defaults.
            Asked::Size(size) => {
                if let Some(bar) = &mut self.screen_bar {
                    bar.size = size;
                }
            }
            Asked::Quality(quality) => {
                if let Some(bar) = &mut self.screen_bar {
                    bar.quality = quality;
                }
            }
            Asked::Pointer(on) => {
                self.settings.screen_pointer = on;
                save(self);
                if let Some(source) = self.screen_bar.as_ref().and_then(|bar| bar.chosen) {
                    self.choose_source(source);
                }
            }
            Asked::Countdown(on) => {
                self.settings.countdown = on;
                save(self);
            }
            Asked::MicrophoneMuted(muted) => {
                self.with_live_screen(|live| live.set_microphone_muted(muted));
            }
            Asked::Folded(folded) => self.with_live_screen(|live| {
                live.folded = folded;
                // Folding closes the camera's picture too, and unfolding doesn't bring it back.
                if folded {
                    live.camera_shown = false;
                }
            }),
            Asked::CameraShown(on) => self.with_live_screen(|live| live.camera_shown = on),
            Asked::Browse => {
                let pick = self.pick_folder;
                if let Some(bar) = &mut self.screen_bar
                    && let Some(picked) = pick(&folder)
                {
                    bar.folder = Some(picked);
                }
            }
        }

        /// Pause, Stop, the time, the microphone and the camera's picture, in the bar and on the
        /// overlay alike.
        fn recording_controls(ui: &mut egui::Ui, palette: &Palette, shown: Shown) -> Option<Asked> {
            let mut asked = None;
            if pause_button(ui, palette, shown.paused) {
                asked = Some(Asked::Pause(!shown.paused));
            }
            if stop_button(ui, palette) {
                asked = Some(Asked::Stop);
            }
            time_label(ui, palette, shown.seconds);
            match shown.microphone_muted {
                Some(muted) => {
                    let (icon, name) = if muted {
                        (icons::MIC_OFF, "Turn the microphone on")
                    } else {
                        (icons::MIC, "Turn the microphone off")
                    };
                    let button = IconButton::new(palette, Kind::Ghost, icon, name);
                    if ui.add(button).on_hover_text(name).clicked() {
                        asked = Some(Asked::MicrophoneMuted(!muted));
                    }
                }
                None => {
                    let button =
                        IconButton::new(palette, Kind::Ghost, icons::MIC_OFF, "No microphone");
                    ui.add_enabled(false, button)
                        .on_disabled_hover_text("No microphone was found when recording began");
                }
            }
            if let Some(on) = shown.camera_shown {
                let (icon, name) = if on {
                    (icons::WEBCAM, "Hide the camera")
                } else {
                    (icons::WEBCAM_OFF, "Show the camera")
                };
                let button = IconButton::new(palette, Kind::Ghost, icon, name);
                if ui.add(button).on_hover_text(name).clicked() {
                    asked = Some(Asked::CameraShown(!on));
                }
            }
            asked
        }

        /// The camera's live picture, under the controls.
        fn camera_window(ui: &mut egui::Ui, palette: &Palette, picture: &TextureHandle) {
            ui.add_space(CAMERA_GAP);
            let width = ui.min_rect().width().max(ui.available_width());
            let (spot, response) =
                ui.allocate_exact_size(vec2(width, camera_picture_height(width)), Sense::hover());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Image, true, "The camera's picture")
            });
            ui.painter().rect_filled(spot, 4.0, palette.stage);
            draw_live_picture(ui.painter(), spot, picture);
        }

        /// The handle the overlay is dragged by. Windows moves the window itself, so it follows
        /// the pointer smoothly and stays where it's dropped.
        fn grip(ui: &mut egui::Ui, palette: &Palette) {
            let (rect, response) =
                ui.allocate_exact_size(vec2(GRIP_WIDTH, CONTROL_SIZE), Sense::drag());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, "Move the recording bar")
            });
            let colour = if response.hovered() || response.dragged() {
                palette.text
            } else {
                palette.text_muted
            };
            icons::paint(
                ui.painter(),
                icons::square_at(rect.center(), 16.0),
                &icons::GRIP_VERTICAL,
                colour,
            );
            if response.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            response.on_hover_cursor(egui::CursorIcon::Grab);
        }

        /// A small window on top of everything, starting near the screen's top right corner, so
        /// the recording can be paused and stopped from inside a game. It keeps itself out of
        /// the recording like BareClip's other windows. The whole window is painted, so no part
        /// of it is left black.
        fn recording_overlay(
            ctx: &egui::Context,
            palette: &Palette,
            shown: Shown,
            at: egui::Pos2,
            camera_picture: Option<TextureHandle>,
        ) -> Option<Asked> {
            let builder = egui::ViewportBuilder::default()
                .with_title("BareClip recording")
                .with_inner_size(overlay_size(shown.folded, shown.camera_shown))
                .with_position(at)
                .with_decorations(false)
                .with_resizable(false)
                .with_always_on_top()
                .with_taskbar(false);
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("recording overlay"),
                builder,
                move |ui, _| {
                    ui.painter().rect(
                        ui.max_rect(),
                        0.0,
                        palette.raised,
                        Stroke::new(1.0, palette.border),
                        egui::StrokeKind::Inside,
                    );
                    Frame::new()
                        .inner_margin(Margin::same(OVERLAY_PADDING))
                        .show(ui, |ui| {
                            let asked = ui
                                .horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                                    grip(ui, palette);
                                    let mut asked = None;
                                    if shown.folded {
                                        if pause_button(ui, palette, shown.paused) {
                                            asked = Some(Asked::Pause(!shown.paused));
                                        }
                                        if stop_button(ui, palette) {
                                            asked = Some(Asked::Stop);
                                        }
                                    } else {
                                        asked = recording_controls(ui, palette, shown);
                                    }
                                    let (icon, name) = if shown.folded {
                                        (icons::CHEVRON_RIGHT, "Show all controls")
                                    } else {
                                        (icons::CHEVRON_LEFT, "Show only Pause and Stop")
                                    };
                                    let fold = IconButton::new(palette, Kind::Ghost, icon, name)
                                        .size(FOLD_WIDTH, 16.0);
                                    if ui.add(fold).on_hover_text(name).clicked() {
                                        asked = Some(Asked::Folded(!shown.folded));
                                    }
                                    asked
                                })
                                .inner;
                            if !shown.folded
                                && shown.camera_shown == Some(true)
                                && let Some(picture) = &camera_picture
                            {
                                camera_window(ui, palette, picture);
                            }
                            asked
                        })
                        .inner
                },
            )
        }
    }
}

/// A menu of the sizes something of `source`'s size can be recorded at: its own, and each
/// named size smaller than it. Gives the one picked.
pub(super) fn size_menu(
    ui: &mut egui::Ui,
    palette: &Palette,
    (shown, source): (RecordingSize, (u32, u32)),
    width: f32,
) -> Option<RecordingSize> {
    let current = text::recording_size_name(shown, Some(source));
    let name = format!("Size: {current}");
    let button = ui.add(components::Dropdown::new(palette, &name, &current, width));
    let menu_width = components::menu_width(&button);
    let mut picked = None;
    egui::Popup::menu(&button).show(|ui| {
        for size in RecordingSize::ALL
            .into_iter()
            .filter(|size| size.fits(source))
        {
            let label = text::recording_size_name(size, Some(source));
            let item = MenuItem::choice(palette, &label, size == shown).width(menu_width);
            if ui.add(item).clicked() {
                picked = Some(size);
            }
        }
    });
    picked
}

/// A menu of the three qualities, named as in the export window. Gives the one picked.
pub(super) fn quality_menu(
    ui: &mut egui::Ui,
    palette: &Palette,
    shown: RecordingQuality,
    width: f32,
) -> Option<RecordingQuality> {
    let current = text::quality_name(shown.into());
    let name = format!("Quality: {current}");
    let button = ui.add(components::Dropdown::new(palette, &name, current, width));
    let menu_width = components::menu_width(&button);
    let mut picked = None;
    egui::Popup::menu(&button).show(|ui| {
        for quality in RecordingQuality::ALL {
            let label = text::quality_name(quality.into());
            let item = MenuItem::choice(palette, label, quality == shown).width(menu_width);
            if ui.add(item).clicked() {
                picked = Some(quality);
            }
        }
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_copy_keeps_its_shape_and_colours() {
        let picture = bareclip_capture::Picture {
            width: 640,
            height: 360,
            bgra: [10, 20, 30, 255].repeat(640 * 360),
            shown_at: 0,
        };
        let small = shrunk(&picture, 320);
        assert_eq!(small.size, [320, 180]);
        assert_eq!(small.pixels[0], Color32::from_rgb(30, 20, 10));
    }

    #[test]
    fn a_small_picture_is_never_made_bigger() {
        let picture = bareclip_capture::Picture {
            width: 100,
            height: 50,
            bgra: [0, 0, 0, 255].repeat(100 * 50),
            shown_at: 0,
        };
        assert_eq!(shrunk(&picture, 320).size, [100, 50]);
    }

    #[test]
    fn the_overlay_starts_near_the_top_right_corner_below_window_buttons() {
        let size = overlay_size(false, None);
        let at = overlay_start(vec2(1920.0, 1080.0), size);
        assert_eq!(at.x + size.x, 1920.0 - OVERLAY_MARGIN);
        assert_eq!(at.y, OVERLAY_TOP);
        assert!(at.y > 48.0, "below a full-screen window's buttons");
        assert_eq!(
            overlay_start(vec2(100.0, 100.0), size).x,
            0.0,
            "never off screen"
        );
    }

    #[test]
    fn the_overlay_grows_for_the_camera_and_folds_down_to_pause_and_stop() {
        let plain = overlay_size(false, None);
        let with_camera = overlay_size(false, Some(false));
        let showing = overlay_size(false, Some(true));
        assert_eq!(with_camera.x, plain.x + CONTROL_SIZE + CONTROL_SPACING);
        assert_eq!(with_camera.y, plain.y);
        assert_eq!(showing.x, with_camera.x);
        let inner = with_camera.x - 2.0 * f32::from(OVERLAY_PADDING);
        assert_eq!(
            showing.y,
            with_camera.y + CAMERA_GAP + camera_picture_height(inner)
        );
        // Folded, it's the same with or without the camera.
        assert_eq!(overlay_size(true, Some(true)), overlay_size(true, None));
        assert!(overlay_size(true, None).x < plain.x);
    }

    #[test]
    fn the_cameras_box_sits_in_the_bottom_right_corner_at_picture_in_pictures_size() {
        let share = camera_box_share((1280, 720), (1920, 1080));
        // 30% of the picture, 4% of the shorter side in from the edges.
        assert!((share.width() - 0.3).abs() < 0.01, "{share:?}");
        assert!((share.height() - 0.3).abs() < 0.01, "{share:?}");
        let margin = 1080.0 * 0.04;
        assert!(
            (share.right() - (1.0 - margin / 1920.0)).abs() < 0.002,
            "{share:?}"
        );
        assert!(
            (share.bottom() - (1.0 - margin / 1080.0)).abs() < 0.002,
            "{share:?}"
        );
    }
}
