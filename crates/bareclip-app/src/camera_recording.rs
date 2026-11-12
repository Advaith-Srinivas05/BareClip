//! Recording from a camera, with the microphone on a track of its own in the same file, so it
//! lands as one video at the playhead. The bar floats over the picture like the microphone's,
//! with a live picture shown mirrored, as people expect to see themselves, while the file keeps
//! the camera's own way round. It can play the video while recording, for reactions, and the
//! recording then lands where the video was when it began.
//!
//! Picking a camera, its size and a microphone is shared with the screen and camera bar, through
//! `CameraPick` and the choice rows below.

use std::any::Any;

use bareclip_audio::Microphone;
use bareclip_capture::{
    Camera, CameraCapture, CameraMode, CameraState, CameraView, Picture, PictureSource,
    ScreenRecording, ScreenSettings, SoundFeed,
};
use bareclip_media::Quality;

use super::recording::{
    GROUP_GAP, HEADPHONES_NOTE, MicSource, local_time, meter, meter_fill, pause_button,
    recording_name, save_in_row, shown_microphone, sound_switch, stop_button, time_label,
    unused_path,
};
use super::screen_recording::shrunk;
use super::*;
use crate::storage::RecordingMarker;

const CAMERA_BAR_WIDTH: f32 = 420.0;
/// How much of the line the camera's size takes, beside the camera's name.
const SIZE_SHARE: f32 = 0.42;
const CAMERA_CHOICE_GAP: f32 = 8.0;
const PREVIEW_HEIGHT: f32 = 150.0;
/// How often the live picture is redrawn, in seconds, smooth enough to see yourself move.
const PREVIEW_EVERY: f64 = 0.04;
const PREVIEW_WIDTH: u32 = 360;
/// The live picture's height in the slim row shown while recording.
const SLIM_PREVIEW_HEIGHT: f32 = 36.0;
const SLIM_METER_WIDTH: f32 = 96.0;
const COUNTDOWN_SECONDS: f64 = 3.0;
const BAR_LIFT: f32 = 16.0;
/// How long to wait for the video to start moving before recording without it.
const START_WAIT_SECONDS: f64 = 1.0;

/// A camera being read, or something standing in for one in tests.
pub(super) struct CameraSource {
    pub view: CameraView,
    /// Whatever keeps the pictures coming, dropped to close the camera.
    pub _keep: Box<dyn Any + Send>,
}

impl PictureSource for CameraSource {
    fn latest(&self) -> Option<(Picture, u64)> {
        self.view.latest()
    }

    fn size(&self) -> (u32, u32) {
        PictureSource::size(&self.view)
    }
}

/// Starts opening a camera, in a size and frame rate if one is given.
pub(super) type OpenCamera = fn(&Camera, Option<CameraMode>) -> CameraSource;
/// Every camera plugged in.
pub(super) type ListCameras = fn() -> Vec<Camera>;

pub(super) fn open_real_camera(camera: &Camera, mode: Option<CameraMode>) -> CameraSource {
    let capture = CameraCapture::start(camera, mode);
    CameraSource {
        view: capture.view(),
        _keep: Box::new(capture),
    }
}

pub(super) fn list_real_cameras() -> Vec<Camera> {
    bareclip_capture::cameras()
}

/// Where the live picture's corners take their colours from: swapped left for right when
/// mirrored, so the picture shows like a mirror without copying it.
pub(super) fn preview_uv(mirrored: bool) -> egui::Rect {
    if mirrored {
        egui::Rect::from_min_max(pos2(1.0, 0.0), pos2(0.0, 1.0))
    } else {
        egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
    }
}

/// Draws a live picture mirrored, as large as fits `spot` and in its middle.
pub(super) fn draw_live_picture(
    painter: &egui::Painter,
    spot: egui::Rect,
    picture: &TextureHandle,
) {
    let size = picture.size_vec2();
    let scale = (spot.width() / size.x).min(spot.height() / size.y);
    let shown = egui::Rect::from_center_size(spot.center(), size * scale);
    painter.image(picture.id(), shown, preview_uv(true), Color32::WHITE);
}

/// The cameras on offer, the one open, and its live picture: what a bar that records a camera
/// needs before it starts.
pub(super) struct CameraPick {
    cameras: Vec<Camera>,
    /// Which of `cameras` is chosen.
    chosen: Option<usize>,
    /// The open camera. A recording takes it over while it runs.
    source: Option<CameraSource>,
    /// A look at the camera that stays while recording, for the live picture.
    view: Option<CameraView>,
    pub(super) preview: Option<TextureHandle>,
    preview_count: u64,
    preview_at: f64,
}

impl CameraPick {
    pub(super) fn new(cameras: Vec<Camera>) -> Self {
        Self {
            cameras,
            chosen: None,
            source: None,
            view: None,
            preview: None,
            preview_count: 0,
            preview_at: 0.0,
        }
    }

    /// Opens `remembered` if it's plugged in, otherwise the first camera, in `mode` if given.
    /// The old camera lets go first, since it may be the same one.
    fn reopen(&mut self, remembered: Option<&str>, mode: Option<CameraMode>, open: OpenCamera) {
        self.source = None;
        self.view = None;
        self.preview = None;
        self.preview_count = 0;
        self.chosen = remembered
            .and_then(|name| self.cameras.iter().position(|camera| camera.name == name))
            .or_else(|| (!self.cameras.is_empty()).then_some(0));
        let Some(camera) = self.chosen.and_then(|index| self.cameras.get(index)) else {
            return;
        };
        let source = open(camera, mode);
        self.view = Some(source.view.clone());
        self.source = Some(source);
    }

    pub(super) fn state(&self) -> CameraState {
        match (&self.view, self.cameras.is_empty()) {
            (_, true) => CameraState::Failed(
                "No camera was found. Plug one in and press Look again.".to_owned(),
            ),
            (Some(view), _) => view.state(),
            (None, _) => CameraState::Starting,
        }
    }

    pub(super) fn is_ready(&self) -> bool {
        matches!(self.state(), CameraState::Running { .. })
    }

    fn chosen_name(&self) -> String {
        self.chosen
            .and_then(|index| self.cameras.get(index))
            .map_or_else(|| "No camera".to_owned(), |camera| camera.name.clone())
    }

    /// The open camera and the mode it runs in, for a recording to take over.
    pub(super) fn take_running(&mut self) -> Option<(CameraSource, CameraMode)> {
        let CameraState::Running { mode, .. } = self.state() else {
            return None;
        };
        Some((self.source.take()?, mode))
    }

    /// Keeps the live picture up to date, a few times a second.
    pub(super) fn refresh(&mut self, ctx: &egui::Context, now: f64) {
        if now - self.preview_at < PREVIEW_EVERY {
            return;
        }
        self.preview_at = now;
        if let Some(view) = &self.view
            && let Some((picture, count)) = view.latest()
            && count != self.preview_count
        {
            self.preview_count = count;
            let image = shrunk(&picture, PREVIEW_WIDTH);
            match &mut self.preview {
                Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                None => {
                    self.preview = Some(ctx.load_texture(
                        "camera-preview",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
            }
        }
    }
}

/// A change asked for in the camera's choices.
pub(super) enum CameraAsk {
    Camera(String),
    Mode(CameraMode),
    LookAgain,
}

/// The Camera and Size lists, or Look again when no camera was found.
pub(super) fn camera_choices(
    ui: &mut egui::Ui,
    palette: &Palette,
    pick: &CameraPick,
    width: f32,
) -> Option<CameraAsk> {
    let mut asked = None;
    if pick.cameras.is_empty() {
        let button = ActionButton::new(palette, Kind::Secondary, "Look again").small();
        if ui.add(button).clicked() {
            asked = Some(CameraAsk::LookAgain);
        }
        return asked;
    }
    // The camera and its size share a line, since each is short.
    let size_width = (width * SIZE_SHARE).round();
    let camera_width = width - size_width - CAMERA_CHOICE_GAP;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = CAMERA_CHOICE_GAP;
        let shown = pick.chosen_name();
        let name = format!("Camera: {shown}");
        let button = ui.add(components::Dropdown::new(
            palette,
            &name,
            &shown,
            camera_width,
        ));
        let menu_width = components::menu_width(&button);
        egui::Popup::menu(&button).show(|ui| {
            for camera in &pick.cameras {
                let item =
                    MenuItem::choice(palette, &camera.name, camera.name == shown).width(menu_width);
                if ui.add(item).clicked() {
                    asked = Some(CameraAsk::Camera(camera.name.clone()));
                }
            }
        });
        let (modes, mode) = match pick.state() {
            CameraState::Running { modes, mode } => (modes, Some(mode)),
            _ => (Vec::new(), None),
        };
        let shown = mode.map_or_else(|| "Waiting for the camera".to_owned(), |mode| mode.label());
        let name = format!("Size: {shown}");
        let button = ui.add_enabled(
            mode.is_some(),
            components::Dropdown::new(palette, &name, &shown, size_width),
        );
        let menu_width = components::menu_width(&button);
        egui::Popup::menu(&button).show(|ui| {
            egui::ScrollArea::vertical()
                .max_height(280.0)
                .show(ui, |ui| {
                    for offered in &modes {
                        let chosen = mode.is_some_and(|mode| mode == *offered);
                        let label = offered.label();
                        let item = MenuItem::choice(palette, &label, chosen).width(menu_width);
                        if ui.add(item).clicked() && !chosen {
                            asked = Some(CameraAsk::Mode(*offered));
                        }
                    }
                });
        });
    });
    asked
}

/// The Microphone list. Gives the name picked, if one was.
pub(super) fn microphone_choice(
    ui: &mut egui::Ui,
    palette: &Palette,
    microphones: &[Microphone],
    shown: &str,
    width: f32,
) -> Option<String> {
    let mut picked = None;
    let name = format!("Microphone: {shown}");
    let button = ui.add(components::Dropdown::new(palette, &name, shown, width));
    let menu_width = components::menu_width(&button);
    egui::Popup::menu(&button).show(|ui| {
        for microphone in microphones {
            let item = MenuItem::choice(palette, &microphone.name, microphone.name == shown)
                .width(menu_width);
            if ui.add(item).clicked() {
                picked = Some(microphone.name.clone());
            }
        }
    });
    picked
}

/// The camera's state in words over its picture's spot, while there is no picture to show.
pub(super) fn camera_state_text(
    ui: &egui::Ui,
    palette: &Palette,
    spot: egui::Rect,
    state: &CameraState,
) {
    let (text, colour) = match state {
        CameraState::Failed(problem) => (problem.as_str(), palette.danger),
        CameraState::Starting => ("Starting the camera", crate::theme::DARK.text),
        CameraState::Running { .. } => return,
    };
    ui.painter().text(
        spot.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(12.0),
        colour,
    );
}

/// What the camera bar is doing.
pub(super) enum CameraStage {
    Waiting,
    /// Counting down, until this moment in the interface's clock.
    Counting(f64),
    /// The file and encoder are open but paused, and the video has been asked to play from
    /// `from`. The recording goes on once the video moves, so the two line up, however long the
    /// encoder took to open. `since` is the first frame of waiting, in case it never moves.
    Starting {
        from: Ticks,
        since: Option<f64>,
        live: Box<LiveCamera>,
    },
    Recording(Box<LiveCamera>),
}

pub(super) struct LiveCamera {
    recording: ScreenRecording,
    path: PathBuf,
    marker: Option<RecordingMarker>,
    /// Where on the timeline the recording's first moment belongs.
    lands_at: Ticks,
    /// The video is playing along, so there's no pausing and stopping pauses it too.
    playing_along: bool,
}

#[cfg(test)]
impl LiveCamera {
    pub(super) fn lands_at(&self) -> Ticks {
        self.lands_at
    }

    pub(super) fn seconds(&self) -> f64 {
        self.recording.seconds()
    }
}

/// The camera recording bar, while it's open.
pub(super) struct CameraBar {
    pick: CameraPick,
    microphones: Vec<Microphone>,
    /// The microphone picked for this recording, starting on Settings' default. `None` is
    /// Windows' default.
    chosen_microphone: Option<String>,
    microphone: Option<Result<MicSource, String>>,
    microphone_level: f32,
    /// A folder picked with Browse for this recording only.
    pub(super) folder: Option<PathBuf>,
    pub(super) stage: CameraStage,
}

impl CameraBar {
    pub(super) fn is_recording(&self) -> bool {
        !matches!(self.stage, CameraStage::Waiting)
    }
}

impl Editor {
    pub(super) fn open_camera_bar(&mut self) {
        let cameras = (self.list_cameras)();
        let microphones = (self.list_microphones)();
        let wanted = self.default_microphone(&microphones);
        let microphone = self
            .settings
            .camera_microphone
            .then(|| (self.open_microphone)(wanted.as_deref()));
        self.camera_bar = Some(CameraBar {
            pick: CameraPick::new(cameras),
            microphones,
            chosen_microphone: wanted,
            microphone,
            microphone_level: 0.0,
            folder: None,
            stage: CameraStage::Waiting,
        });
        self.open_chosen_camera();
    }

    /// Settings' default microphone, if it's plugged in, or `None` for Windows' default. Every
    /// recording window starts on it.
    pub(super) fn default_microphone(&self, microphones: &[Microphone]) -> Option<String> {
        self.settings.microphone.clone().filter(|name| {
            microphones
                .iter()
                .any(|microphone| microphone.name == *name)
        })
    }

    /// The camera choices of whichever bar is open: the camera's, or the screen and camera's.
    pub(super) fn camera_pick(&mut self) -> Option<&mut CameraPick> {
        if let Some(bar) = &mut self.camera_bar {
            return Some(&mut bar.pick);
        }
        self.screen_bar.as_mut().and_then(|bar| bar.camera.as_mut())
    }

    /// Opens the camera picked before if it's plugged in, otherwise the first, in the size and
    /// frame rate picked for it.
    pub(super) fn open_chosen_camera(&mut self) {
        let open = self.open_camera;
        let remembered = self.settings.camera.clone();
        let mode = self.settings.camera_mode.map(CameraMode::from);
        if let Some(pick) = self.camera_pick() {
            pick.reopen(remembered.as_deref(), mode, open);
        }
    }

    /// Acts on a change asked for in the camera's choices.
    pub(super) fn camera_asked(&mut self, asked: CameraAsk) {
        match asked {
            CameraAsk::Camera(name) => {
                if self.settings.camera.as_ref() != Some(&name) {
                    // A size picked for one camera means nothing to another.
                    self.settings.camera_mode = None;
                }
                self.settings.camera = Some(name);
                self.save_settings();
            }
            CameraAsk::Mode(mode) => {
                self.settings.camera_mode = Some(mode.into());
                self.save_settings();
            }
            CameraAsk::LookAgain => {
                let cameras = (self.list_cameras)();
                if let Some(pick) = self.camera_pick() {
                    pick.cameras = cameras;
                }
            }
        }
        self.open_chosen_camera();
    }

    fn set_camera_microphone(&mut self, on: bool) {
        self.settings.camera_microphone = on;
        self.save_settings();
        let open = self.open_microphone;
        if let Some(bar) = &mut self.camera_bar {
            let opened = on.then(|| open(bar.chosen_microphone.as_deref()));
            // The old one stops being listened to as it's replaced.
            bar.microphone = opened;
            bar.microphone_level = 0.0;
        }
    }

    /// Picks a microphone for this recording only, and listens to it if it's switched on.
    /// Settings' default stays as it is.
    fn choose_camera_microphone(&mut self, name: String) {
        let open = self.open_microphone;
        let on = self.settings.camera_microphone;
        if let Some(bar) = &mut self.camera_bar {
            if on {
                // The old one stops being listened to as it's replaced.
                bar.microphone = Some(open(Some(&name)));
                bar.microphone_level = 0.0;
            }
            bar.chosen_microphone = Some(name);
        }
    }

    /// Opens the file and encoder, after any countdown, as belonging at `lands_at` on the
    /// timeline. With `along` it opens paused, sets the video playing, and goes on once the video
    /// moves, because opening an encoder can take a second the video would play through.
    fn start_camera_recording(&mut self, lands_at: Ticks, along: bool) {
        let default_folder = self.recordings_folder();
        let microphone_on = self.settings.camera_microphone;
        let Some(bar) = &mut self.camera_bar else {
            return;
        };
        let Some((source, mode)) = bar.pick.take_running() else {
            bar.stage = CameraStage::Waiting;
            return;
        };
        let folder = bar.folder.clone().unwrap_or(default_folder);
        let mut sounds = Vec::new();
        if microphone_on
            && let Some(Ok(microphone)) = &mut bar.microphone
            && let Some(queue) = microphone.queue.take()
        {
            sounds.push(SoundFeed {
                queue,
                rate: microphone.rate,
                name: "Microphone".to_owned(),
                muted: false,
            });
        }
        if let Err(error) = std::fs::create_dir_all(&folder) {
            self.notice = Some(format!(
                "The recording could not start, because {} could not be made. {error}",
                folder.display()
            ));
            self.camera_bar = None;
            return;
        }
        let path = unused_path(&folder, &recording_name("Camera", local_time(), "mp4"));
        let settings = camera_settings(mode);
        let start = if along {
            ScreenRecording::start_paused
        } else {
            ScreenRecording::start
        };
        match start(&path, Box::new(source), sounds, settings) {
            Ok(recording) => {
                let marker = self
                    .app_folder
                    .as_ref()
                    .and_then(|app| RecordingMarker::start(app, &path).ok());
                let live = Box::new(LiveCamera {
                    recording,
                    path,
                    marker,
                    lands_at,
                    playing_along: along,
                });
                if let Some(bar) = &mut self.camera_bar {
                    bar.stage = if along {
                        CameraStage::Starting {
                            from: lands_at,
                            since: None,
                            live,
                        }
                    } else {
                        CameraStage::Recording(live)
                    };
                }
                if along && !self.preview.is_playing() {
                    self.toggle_playback();
                }
            }
            Err(error) => {
                self.notice = Some(format!("The recording could not start. {error}"));
                self.camera_bar = None;
            }
        }
    }

    /// Pauses the video if the camera bar set it playing.
    fn stop_playing_along(&mut self) {
        let along = match self.camera_bar.as_ref().map(|bar| &bar.stage) {
            Some(CameraStage::Starting { .. }) => true,
            Some(CameraStage::Recording(live)) => live.playing_along,
            _ => false,
        };
        if along && self.preview.is_playing() {
            self.toggle_playback();
        }
    }

    /// Finishes the file and adds it to the project. Stopped before it went on, nothing was
    /// recorded, so the empty file is removed.
    pub(super) fn stop_camera_recording(&mut self, ctx: &egui::Context) {
        self.stop_playing_along();
        let Some(bar) = self.camera_bar.take() else {
            return;
        };
        let (live, began) = match bar.stage {
            CameraStage::Recording(live) => (live, true),
            CameraStage::Starting { live, .. } => (live, false),
            _ => return,
        };
        let LiveCamera {
            recording,
            path,
            marker,
            lands_at,
            ..
        } = *live;
        let finished = recording.stop();
        if !began {
            if let Some(marker) = marker {
                marker.finish();
            }
            let _ = std::fs::remove_file(&path);
            return;
        }
        match finished {
            Ok(_) => {
                if let Some(marker) = marker {
                    marker.finish();
                }
                self.add_recording(ctx, &path, lands_at);
            }
            Err(error) => {
                self.notice = Some(format!(
                    "The recording could not be finished. What was recorded is in {}. {error}",
                    path.display()
                ));
            }
        }
    }

    /// Moves the bar on: the countdown ending, and the video starting to move.
    fn advance_camera_bar(&mut self, now: f64) {
        let Some(bar) = &mut self.camera_bar else {
            return;
        };
        match &mut bar.stage {
            CameraStage::Counting(until) if now >= *until => {
                let from = self.position();
                let along = self.settings.play_while_recording && self.can_play_along();
                self.start_camera_recording(from, along);
            }
            CameraStage::Starting { since: None, .. } => {
                // Counted from the first frame after the encoder opened, which may have taken a
                // while.
                if let CameraStage::Starting { since, .. } = &mut bar.stage {
                    *since = Some(now);
                }
            }
            CameraStage::Starting {
                from,
                since: Some(since),
                ..
            } => {
                let (from, since) = (*from, *since);
                let position = self.position();
                if position == from && now - since < START_WAIT_SECONDS {
                    return;
                }
                let playing = self.preview.is_playing();
                let Some(bar) = &mut self.camera_bar else {
                    return;
                };
                if let CameraStage::Starting { mut live, .. } =
                    std::mem::replace(&mut bar.stage, CameraStage::Waiting)
                {
                    live.recording.set_paused(false);
                    live.lands_at = position;
                    live.playing_along = playing;
                    bar.stage = CameraStage::Recording(live);
                }
            }
            _ => {}
        }
    }

    /// Keeps the live picture and the meter up to date.
    fn refresh_camera_bar(&mut self, ctx: &egui::Context, now: f64) {
        let Some(bar) = &mut self.camera_bar else {
            return;
        };
        if let Some(Ok(microphone)) = &bar.microphone {
            bar.microphone_level =
                (bar.microphone_level * 0.85).max(meter_fill(microphone.level.take()));
        }
        bar.pick.refresh(ctx, now);
    }

    /// The camera recording bar, floating over the bottom of the picture while it's open.
    pub(super) fn camera_bar_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        if self.camera_bar.is_none() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(33));
        let now = ctx.input(|input| input.time);
        self.refresh_camera_bar(ctx, now);
        self.advance_camera_bar(now);
        let Some(bar) = &self.camera_bar else {
            return;
        };
        enum Asked {
            Nothing,
            Record,
            Cancel,
            Stop,
            Pause(bool),
            Camera(CameraAsk),
            Microphone(bool),
            ChooseMicrophone(String),
            PlayAlong(bool),
            Countdown(bool),
            Browse,
        }
        let mut asked = Asked::Nothing;
        let state = bar.pick.state();
        // While recording: its time, and whether it can pause.
        let recording = match &bar.stage {
            CameraStage::Starting { .. } => Some((0.0, false, false)),
            CameraStage::Recording(live) => Some((
                live.recording.seconds(),
                live.recording.is_paused(),
                !live.playing_along,
            )),
            _ => None,
        };
        let counting = match bar.stage {
            CameraStage::Counting(until) => Some((until - now).ceil().max(1.0) as u32),
            _ => None,
        };
        let settings = self.settings.clone();
        let shown_mic = shown_microphone(bar.chosen_microphone.as_ref(), &bar.microphones);
        let folder = bar
            .folder
            .clone()
            .unwrap_or_else(|| self.recordings_folder());
        let stage = self.stage_rect;
        let live_picture = |ui: &mut egui::Ui, spot: egui::Rect| {
            if let Some(texture) = &bar.pick.preview {
                draw_live_picture(ui.painter(), spot, texture);
            }
        };
        egui::Area::new(egui::Id::unique("camera_bar"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(stage.center().x, stage.bottom() - BAR_LIFT))
            .constrain(true)
            .show(ctx, |ui| {
                // While recording it's one slim row, so it covers as little of the video as it can.
                if let Some((seconds, paused, can_pause)) = recording {
                    dialog_frame(ctx)
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let width = SLIM_PREVIEW_HEIGHT * 16.0 / 9.0;
                                let (spot, _) = ui.allocate_exact_size(
                                    vec2(width, SLIM_PREVIEW_HEIGHT),
                                    Sense::hover(),
                                );
                                ui.painter().rect_filled(spot, 4.0, palette.stage);
                                live_picture(ui, spot);
                                if can_pause && pause_button(ui, palette, paused) {
                                    asked = Asked::Pause(!paused);
                                }
                                if stop_button(ui, palette) {
                                    asked = Asked::Stop;
                                }
                                time_label(ui, palette, seconds);
                                if let CameraState::Failed(problem) = &state {
                                    ui.colored_label(palette.danger, "Camera lost")
                                        .on_hover_text(problem);
                                } else if settings.camera_microphone {
                                    ui.allocate_ui(vec2(SLIM_METER_WIDTH, 10.0), |ui| {
                                        meter(ui, palette, bar.microphone_level);
                                    });
                                }
                                ui.add_space(4.0);
                            });
                        });
                    return;
                }
                dialog_frame(ctx).show(ui, |ui| {
                    ui.set_width(CAMERA_BAR_WIDTH);
                    components::dialog_title(ui, &icons::WEBCAM, palette.accent, "Record camera");
                    ui.add_space(12.0);
                    let width = ui.available_width();
                    let (spot, _) =
                        ui.allocate_exact_size(vec2(width, PREVIEW_HEIGHT), Sense::hover());
                    ui.painter().rect_filled(spot, 6.0, palette.stage);
                    camera_state_text(ui, palette, spot, &state);
                    if matches!(state, CameraState::Running { .. }) {
                        live_picture(ui, spot);
                    }
                    if let Some(left) = counting {
                        // The number sits over the live picture, so you can still see yourself.
                        ui.painter().text(
                            spot.center(),
                            egui::Align2::CENTER_CENTER,
                            left.to_string(),
                            fonts::strong(56.0),
                            crate::theme::DARK.text,
                        );
                        ui.add_space(12.0);
                        components::dialog_buttons(ui, |ui| {
                            if ui.add(dialog_button(palette, "Cancel")).clicked() {
                                asked = Asked::Cancel;
                            }
                        });
                        return;
                    }
                    ui.add_space(10.0);
                    if let Some(choice) = camera_choices(ui, palette, &bar.pick, width) {
                        asked = Asked::Camera(choice);
                    }
                    ui.add_space(GROUP_GAP);
                    components::settings_group(ui, palette, "Sound", |ui| {
                        if sound_switch(
                            ui,
                            palette,
                            ("Record the microphone", settings.camera_microphone),
                            bar.microphone.as_ref(),
                            bar.microphone_level,
                        ) {
                            asked = Asked::Microphone(!settings.camera_microphone);
                        }
                        // Shown even while it's off, so it can be picked before switching it on.
                        components::settings_wide_control(ui, |ui, width| {
                            if let Some(name) =
                                microphone_choice(ui, palette, &bar.microphones, &shown_mic, width)
                            {
                                asked = Asked::ChooseMicrophone(name);
                            }
                        });
                    });
                    ui.add_space(GROUP_GAP);
                    components::settings_group(ui, palette, "Options", |ui| {
                        let along = settings.play_while_recording;
                        let row = SwitchRow::new(palette, "Play the video while recording", along)
                            .compact();
                        if ui.add(row).clicked() {
                            asked = Asked::PlayAlong(!along);
                        }
                        if along && settings.camera_microphone {
                            components::settings_note(ui, palette, HEADPHONES_NOTE, None);
                        }
                        components::settings_divider(ui, palette);
                        let row = SwitchRow::new(palette, "Count down from 3", settings.countdown)
                            .compact();
                        if ui.add(row).clicked() {
                            asked = Asked::Countdown(!settings.countdown);
                        }
                    });
                    ui.add_space(8.0);
                    if save_in_row(ui, palette, &folder) {
                        asked = Asked::Browse;
                    }
                    ui.add_space(12.0);
                    components::dialog_buttons(ui, |ui| {
                        let record = ActionButton::new(palette, Kind::Primary, "Start recording")
                            .icon(icons::VIDEO)
                            .width(DIALOG_BUTTON_WIDTH * 1.5);
                        let ready = matches!(state, CameraState::Running { .. });
                        if ui.add_enabled(ready, record).clicked() {
                            asked = Asked::Record;
                        }
                        if ui.add(dialog_button(palette, "Cancel")).clicked() {
                            asked = Asked::Cancel;
                        }
                    });
                });
            });
        // Escape only closes it before recording starts. Recording is only ever stopped by Stop.
        if recording.is_none()
            && matches!(asked, Asked::Nothing)
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            asked = Asked::Cancel;
        }
        match asked {
            Asked::Nothing => {}
            Asked::Record => {
                let wait = if self.settings.countdown {
                    COUNTDOWN_SECONDS
                } else {
                    0.0
                };
                if let Some(bar) = &mut self.camera_bar {
                    bar.stage = CameraStage::Counting(now + wait);
                }
                self.advance_camera_bar(now);
            }
            Asked::Cancel => self.camera_bar = None,
            Asked::Stop => self.stop_camera_recording(ctx),
            Asked::Pause(paused) => {
                if let Some(CameraBar {
                    stage: CameraStage::Recording(live),
                    ..
                }) = &self.camera_bar
                {
                    live.recording.set_paused(paused);
                }
            }
            Asked::Camera(choice) => self.camera_asked(choice),
            Asked::Microphone(on) => self.set_camera_microphone(on),
            Asked::ChooseMicrophone(name) => self.choose_camera_microphone(name),
            Asked::PlayAlong(on) => {
                self.settings.play_while_recording = on;
                self.save_settings();
            }
            Asked::Countdown(on) => {
                self.settings.countdown = on;
                self.save_settings();
            }
            Asked::Browse => {
                if let Some(picked) = (self.pick_folder)(&folder)
                    && let Some(bar) = &mut self.camera_bar
                {
                    bar.folder = Some(picked);
                }
            }
        }
    }
}

/// How a camera is recorded: at its own size and frame rate.
pub(super) fn camera_settings(mode: CameraMode) -> ScreenSettings {
    ScreenSettings {
        frame_rate: mode.whole_frame_rate(),
        size: Some((mode.width, mode.height)),
        quality: Quality::Balanced,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_live_picture_is_mirrored_by_swapping_its_sides() {
        let mirrored = preview_uv(true);
        assert_eq!(mirrored.left_top(), pos2(1.0, 0.0));
        assert_eq!(mirrored.right_bottom(), pos2(0.0, 1.0));
        let plain = preview_uv(false);
        assert_eq!(plain.left_top(), pos2(0.0, 0.0));
        assert_eq!(plain.right_bottom(), pos2(1.0, 1.0));
    }
}
