//! Recording straight into the project: the Record panel, the recording bar, and adding what was
//! recorded. The microphone is here, and the screen in `screen_recording.rs`. A recording is a real file in the recordings folder, and
//! lands in My media and on the timeline at the playhead when it stops.

use std::any::Any;

use bareclip_audio::{Level, Microphone, MicrophoneInput, SoundRecording};
use bareclip_core::tracks::TrackTarget;
use bareclip_media::SoundLabel;
use rtrb::Consumer;

use super::*;
use crate::storage::{CutShort, RecordingMarker};

/// How long the countdown before a recording lasts, in seconds.
const COUNTDOWN_SECONDS: f64 = 3.0;
const RECORD_BAR_WIDTH: f32 = 400.0;
const METER_HEIGHT: f32 = 10.0;
/// How wide the meter is in the slim bar shown while recording.
const SLIM_METER_WIDTH: f32 = 120.0;
/// The quietest sound the meter shows, in decibels. Anything quieter is an empty meter.
const METER_FLOOR_DB: f32 = -60.0;
/// How much of the meter's reading stays from one frame to the next, so it falls smoothly.
const METER_FALL: f32 = 0.85;
/// How long to wait for the video to start moving before recording without it.
const START_WAIT_SECONDS: f64 = 1.0;
/// How far up from the picture's bottom edge the bar sits.
const BAR_LIFT: f32 = 16.0;
/// How often the bar redraws while it's open, for the meter and the running time.
const BAR_REDRAW: Duration = Duration::from_millis(33);
const FOLDER_NAME: &str = "BareClip recordings";
/// How wide the running time is kept, so the buttons beside it don't move as it grows.
pub(super) const TIME_WIDTH: f32 = 72.0;

/// A microphone being listened to, or something standing in for one in tests.
pub(super) struct MicSource {
    pub queue: Option<Consumer<f32>>,
    pub rate: u32,
    pub level: Arc<Level>,
    /// Whatever keeps the sound coming, dropped when the bar closes.
    pub _keep: Box<dyn Any>,
}

/// Opens a microphone by name, or the default for `None`.
pub(super) type OpenMicrophone = fn(Option<&str>) -> Result<MicSource, String>;
pub(super) type ListMicrophones = fn() -> Vec<Microphone>;
/// Asks where to save, starting in the folder given. Tests replace it, since the real one waits
/// for a person.
pub(super) type PickFolder = fn(&Path) -> Option<PathBuf>;

pub(super) fn pick_real_folder(from: &Path) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Save recordings in")
        .set_directory(from)
        .pick_folder()
}

/// A recording's running time in whole seconds, such as `0:07`, or `1:02:07` past an hour.
pub(super) fn recording_time(seconds: f64) -> String {
    let whole = seconds.max(0.0).floor() as u64;
    let (hours, minutes, rest) = (whole / 3600, whole / 60 % 60, whole % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{rest:02}")
    } else {
        format!("{minutes}:{rest:02}")
    }
}

/// The running time, in a space that stays the same width as it grows.
pub(super) fn time_label(ui: &mut egui::Ui, palette: &Palette, seconds: f64) {
    let time = recording_time(seconds);
    let (rect, response) = ui.allocate_exact_size(vec2(TIME_WIDTH, 32.0), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &time));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        &time,
        fonts::strong(16.0),
        palette.text,
    );
}

/// Pause while recording, and play to carry on while paused. Says whether it was clicked.
pub(super) fn pause_button(ui: &mut egui::Ui, palette: &Palette, paused: bool) -> bool {
    let (name, icon) = if paused {
        ("Resume recording", icons::PLAY)
    } else {
        ("Pause recording", icons::PAUSE)
    };
    let button = IconButton::new(palette, Kind::Ghost, icon, name).solid();
    ui.add(button).on_hover_text(name).clicked()
}

/// Stop, as a red square. Says whether it was clicked.
pub(super) fn stop_button(ui: &mut egui::Ui, palette: &Palette) -> bool {
    let button = IconButton::new(palette, Kind::Ghost, icons::SQUARE, "Stop")
        .solid()
        .tint(palette.danger);
    ui.add(button).on_hover_text("Stop recording").clicked()
}

/// Where the next recording is saved, with Browse to pick another folder for it. Says whether
/// Browse was clicked.
pub(super) fn save_in_row(ui: &mut egui::Ui, palette: &Palette, folder: &Path) -> bool {
    let mut browse = false;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 96.0).max(80.0);
        ui.allocate_ui_with_layout(
            vec2(width, 26.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                ui.colored_label(palette.text_muted, format!("Save in {}", folder.display()))
                    .on_hover_text(folder.display().to_string());
            },
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let button = ActionButton::new(palette, Kind::Secondary, "Browse")
                .icon(icons::FOLDER_OPEN)
                .small();
            browse = ui.add(button).clicked();
        });
    });
    browse
}

pub(super) fn open_real_microphone(name: Option<&str>) -> Result<MicSource, String> {
    let mut input = MicrophoneInput::open(name)?;
    Ok(MicSource {
        queue: input.take_queue(),
        rate: input.sample_rate(),
        level: input.level(),
        _keep: Box::new(input),
    })
}

/// What the recording bar is doing.
pub(super) enum BarStage {
    /// Showing the meter, waiting for Record.
    Waiting,
    /// Counting down, until this moment in the interface's clock.
    Counting(f64),
    /// The video has been asked to play from `from`, and the recording starts once it moves, so
    /// the voice lines up with the picture. `since` is when it was asked, in case it never does.
    Starting {
        from: Ticks,
        since: f64,
    },
    Recording(Box<LiveRecording>),
}

pub(super) struct LiveRecording {
    recording: SoundRecording,
    path: PathBuf,
    marker: Option<RecordingMarker>,
    /// Where on the timeline the recording's first moment belongs.
    lands_at: Ticks,
    /// The video is playing along, so stopping pauses it too.
    playing_along: bool,
}

#[cfg(test)]
impl LiveRecording {
    pub(super) fn lands_at(&self) -> Ticks {
        self.lands_at
    }

    pub(super) fn seconds(&self) -> f64 {
        self.recording.seconds()
    }
}

/// The recording bar, while it's open.
pub(super) struct RecordBar {
    microphones: Vec<Microphone>,
    source: Result<MicSource, String>,
    shown_level: f32,
    /// A folder picked with Browse for this recording only.
    pub(super) folder: Option<PathBuf>,
    pub(super) stage: BarStage,
}

impl RecordBar {
    pub(super) fn is_recording(&self) -> bool {
        !matches!(self.stage, BarStage::Waiting)
    }

    /// Why the microphone can't be used, if it can't.
    pub(super) fn problem(&self) -> Option<&str> {
        self.source.as_ref().err().map(String::as_str)
    }
}

/// The level meter, for the screen recording bar too.
pub(super) fn meter(ui: &mut egui::Ui, palette: &Palette, fill: f32) {
    level_meter(ui, palette, fill);
}

/// How full the meter is for a level from 0 to 1, on a decibel scale, since that's how loudness
/// is heard.
pub(super) fn meter_fill(level: f32) -> f32 {
    if level <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * level.log10();
    ((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0)
}

/// The microphone to show as chosen: the one picked before if it's still there, otherwise the
/// first, which is Windows' default.
pub(super) fn shown_microphone(chosen: Option<&String>, microphones: &[Microphone]) -> String {
    chosen
        .filter(|name| {
            microphones
                .iter()
                .any(|microphone| &microphone.name == *name)
        })
        .cloned()
        .or_else(|| {
            microphones
                .first()
                .map(|microphone| microphone.name.clone())
        })
        .unwrap_or_else(|| "No microphone".to_owned())
}

/// A recording's file name, such as `Microphone 2026-11-11 14-05-32.m4a`.
pub(super) fn recording_name(
    kind: &str,
    (year, month, day, hour, minute, second): LocalTime,
    extension: &str,
) -> String {
    format!("{kind} {year:04}-{month:02}-{day:02} {hour:02}-{minute:02}-{second:02}.{extension}")
}

/// `name` in `folder`, with ` (2)` and so on added if a file already has it.
pub(super) fn unused_path(folder: &Path, name: &str) -> PathBuf {
    let path = folder.join(name);
    if !path.exists() {
        return path;
    }
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
    (2..)
        .map(|count| folder.join(format!("{stem} ({count}).{extension}")))
        .find(|path| !path.exists())
        .unwrap_or(path)
}

/// Year, month, day, hour, minute and second.
pub(super) type LocalTime = (u16, u16, u16, u16, u16, u16);

pub(super) fn local_time() -> LocalTime {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: `GetLocalTime` only fills in the structure it is given.
    let now = unsafe {
        let mut now = std::mem::zeroed();
        GetLocalTime(&mut now);
        now
    };
    (
        now.wYear,
        now.wMonth,
        now.wDay,
        now.wHour,
        now.wMinute,
        now.wSecond,
    )
}

/// Windows' Videos folder, wherever the user has moved it.
fn videos_folder() -> Option<PathBuf> {
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Videos, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
    };
    // SAFETY: Windows gives back a string of its own, which is read up to its end and then freed
    // as it asks, whether or not the call worked.
    unsafe {
        let mut path = std::ptr::null_mut();
        let result = SHGetKnownFolderPath(
            &FOLDERID_Videos,
            KF_FLAG_DEFAULT as u32,
            std::ptr::null_mut(),
            &mut path,
        );
        let found = (result >= 0 && !path.is_null()).then(|| {
            let length = (0..).take_while(|&index| *path.add(index) != 0).count();
            let wide = std::slice::from_raw_parts(path, length);
            PathBuf::from(String::from_utf16_lossy(wide))
        });
        CoTaskMemFree(path.cast());
        found
    }
}

/// Where recordings go when Settings doesn't say.
pub(super) fn default_recordings_folder() -> PathBuf {
    videos_folder()
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Videos")))
        .unwrap_or_else(std::env::temp_dir)
        .join(FOLDER_NAME)
}

impl Editor {
    /// Where the next recording is saved. A test editor, with no settings of its own, never
    /// writes to the real Videos folder.
    pub(super) fn recordings_folder(&self) -> PathBuf {
        match (&self.settings.recordings_folder, &self.app_folder) {
            (Some(folder), _) => folder.clone(),
            (None, Some(_)) => default_recordings_folder(),
            (None, None) => std::env::temp_dir().join("bareclip-test-recordings"),
        }
    }

    /// The Record panel, in the left panel: one card for each way of recording.
    pub(super) fn record_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let busy = self
            .record_bar
            .as_ref()
            .is_some_and(RecordBar::is_recording)
            || self
                .screen_bar
                .as_ref()
                .is_some_and(super::screen_recording::ScreenBar::is_recording)
            || self
                .camera_bar
                .as_ref()
                .is_some_and(super::camera_recording::CameraBar::is_recording);
        let screen = components::choice_card(
            ui,
            palette,
            &icons::MONITOR,
            "Screen",
            "Record a screen or a window, with the PC's sound and your microphone",
            false,
        );
        if screen.clicked() && !busy {
            self.record_bar = None;
            self.camera_bar = None;
            self.open_screen_bar(false);
        }
        ui.add_space(6.0);
        let both = components::choice_card(
            ui,
            palette,
            &icons::PICTURE_IN_PICTURE_2,
            "Screen and camera",
            "Record a screen with your webcam in a corner, landing as separate clips",
            false,
        );
        if both.clicked() && !busy {
            self.record_bar = None;
            self.camera_bar = None;
            self.open_screen_bar(true);
        }
        ui.add_space(6.0);
        let camera = components::choice_card(
            ui,
            palette,
            &icons::WEBCAM,
            "Camera",
            "Record yourself with a webcam and your microphone, as a video at the playhead",
            false,
        );
        if camera.clicked() && !busy {
            self.record_bar = None;
            self.screen_bar = None;
            self.open_camera_bar();
        }
        ui.add_space(6.0);
        let card = components::choice_card(
            ui,
            palette,
            &icons::MIC,
            "Microphone",
            "Record your voice, or any microphone, onto a sound row at the playhead",
            false,
        );
        if card.clicked() && !busy {
            self.screen_bar = None;
            self.camera_bar = None;
            self.open_record_bar();
        }
    }

    pub(super) fn open_record_bar(&mut self) {
        let microphones = (self.list_microphones)();
        let wanted = self.settings.microphone.as_deref().filter(|name| {
            microphones
                .iter()
                .any(|microphone| microphone.name == *name)
        });
        self.record_bar = Some(RecordBar {
            source: (self.open_microphone)(wanted),
            microphones,
            shown_level: 0.0,
            folder: None,
            stage: BarStage::Waiting,
        });
    }

    fn microphone_name(&self) -> String {
        let microphones = self
            .record_bar
            .as_ref()
            .map_or(&[][..], |bar| &bar.microphones[..]);
        shown_microphone(self.settings.microphone.as_ref(), microphones)
    }

    fn choose_microphone(&mut self, name: String) {
        self.settings.microphone = Some(name.clone());
        self.save_settings();
        if let Some(bar) = &mut self.record_bar {
            // The old one stops being listened to as it's replaced.
            bar.source = (self.open_microphone)(Some(&name));
            bar.shown_level = 0.0;
        }
    }

    /// Starts writing the file, after any countdown, as belonging at `lands_at` on the timeline.
    fn start_recording(&mut self, lands_at: Ticks, playing_along: bool) {
        let default = self.recordings_folder();
        let Some(bar) = &mut self.record_bar else {
            return;
        };
        let folder = bar.folder.clone().unwrap_or(default);
        let source = match &mut bar.source {
            Ok(source) => source,
            Err(_) => return,
        };
        let Some(queue) = source.queue.take() else {
            return;
        };
        if let Err(error) = std::fs::create_dir_all(&folder) {
            self.notice = Some(format!(
                "The recording could not start, because {} could not be made. {error}",
                folder.display()
            ));
            self.record_bar = None;
            return;
        }
        let path = unused_path(&folder, &recording_name("Microphone", local_time(), "m4a"));
        let label = SoundLabel {
            title: Some("Microphone".to_owned()),
            language: None,
        };
        match SoundRecording::start(&path, queue, source.rate, label) {
            Ok(recording) => {
                let marker = self
                    .app_folder
                    .as_ref()
                    .and_then(|app| RecordingMarker::start(app, &path).ok());
                bar.stage = BarStage::Recording(Box::new(LiveRecording {
                    recording,
                    path,
                    marker,
                    lands_at,
                    playing_along,
                }));
            }
            Err(error) => {
                self.notice = Some(format!("The recording could not start. {error}"));
                self.record_bar = None;
            }
        }
    }

    /// Finishes the file and adds it to the project.
    pub(super) fn stop_recording(&mut self, ctx: &egui::Context) {
        let Some(bar) = self.record_bar.take() else {
            return;
        };
        if matches!(bar.stage, BarStage::Starting { .. }) && self.preview.is_playing() {
            self.toggle_playback();
        }
        let BarStage::Recording(live) = bar.stage else {
            return;
        };
        let LiveRecording {
            recording,
            path,
            marker,
            lands_at,
            playing_along,
        } = *live;
        if playing_along && self.preview.is_playing() {
            self.toggle_playback();
        }
        match recording.stop() {
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

    /// Puts a recorded file in My media and on a new row at `at`, moving nothing, as one step to
    /// undo: a sound row for sound, and a picture row above everything for a video.
    pub(super) fn add_recording(&mut self, ctx: &egui::Context, path: &Path, at: Ticks) {
        let mut entry = match self.load(ctx, path) {
            Ok(entry) => entry,
            Err(error) => {
                let name = path
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                self.notice = Some(format!("{name} could not be opened. {error}"));
                return;
            }
        };
        // Made by BareClip, so it can be deleted from My media.
        entry.recorded = true;
        let id = entry.id;
        self.media.push(entry);
        let Some(range) = self.full_range(id) else {
            return;
        };
        if range.duration().0 <= 0 {
            self.notice = Some("Nothing was recorded.".to_owned());
            return;
        }
        self.record();
        let sound_only = self.is_sound_only(id);
        let tracks = &mut self.project.tracks;
        self.selected = Some(if sound_only {
            let row = TrackTarget::New(tracks.picture_rows());
            tracks.place_sound(id, range, at, row)
        } else {
            tracks.place(id, range, at, TrackTarget::New(0))
        });
        self.settle_canvas();
        self.after_clip_added();
    }

    /// Whether the video can play from where the playhead is.
    pub(super) fn can_play_along(&self) -> bool {
        let position = self.position();
        !self.project.tracks.is_empty() && position < self.project.tracks.duration()
    }

    /// Moves the bar on: the countdown ending, and the video starting to move.
    fn advance_record_bar(&mut self, now: f64) {
        let Some(bar) = &self.record_bar else {
            return;
        };
        match bar.stage {
            BarStage::Counting(until) if now >= until => {
                let from = self.position();
                if self.settings.play_while_recording && self.can_play_along() {
                    if !self.preview.is_playing() {
                        self.toggle_playback();
                    }
                    if let Some(bar) = &mut self.record_bar {
                        bar.stage = BarStage::Starting { from, since: now };
                    }
                } else {
                    self.start_recording(from, false);
                }
            }
            BarStage::Starting { from, since } => {
                let position = self.position();
                if position != from || now - since >= START_WAIT_SECONDS {
                    self.start_recording(position, self.preview.is_playing());
                }
            }
            _ => {}
        }
    }

    /// The recording bar, floating over the bottom of the picture while it's open, so the video
    /// can be watched and played while recording.
    pub(super) fn record_bar_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        if self.record_bar.is_none() {
            return;
        }
        ctx.request_repaint_after(BAR_REDRAW);
        let now = ctx.input(|input| input.time);
        if let Some(bar) = &mut self.record_bar
            && let Ok(source) = &bar.source
        {
            let heard = source.level.take();
            bar.shown_level = (bar.shown_level * METER_FALL).max(meter_fill(heard));
        }
        self.advance_record_bar(now);
        let Some(bar) = &self.record_bar else {
            return;
        };
        let level = bar.shown_level;
        let problem = bar.problem().map(str::to_owned);
        let microphones: Vec<String> = bar
            .microphones
            .iter()
            .map(|microphone| microphone.name.clone())
            .collect();
        let shown_microphone = self.microphone_name();
        let folder = bar
            .folder
            .clone()
            .unwrap_or_else(|| self.recordings_folder());
        let countdown = self.settings.countdown;
        let play_along = self.settings.play_while_recording;
        enum Asked {
            Nothing,
            Record,
            Cancel,
            Stop,
            Pause(bool),
            Microphone(String),
            Countdown(bool),
            PlayAlong(bool),
            Browse,
        }
        let mut asked = Asked::Nothing;
        // What's shown: the countdown, a recording's time and whether it can pause, or neither.
        let (counting, recording) = match &bar.stage {
            BarStage::Waiting => (None, None),
            BarStage::Counting(until) => (Some((until - now).ceil().max(1.0) as u32), None),
            BarStage::Starting { .. } => (None, Some((0.0, false, false))),
            BarStage::Recording(live) => (
                None,
                Some((
                    live.recording.seconds(),
                    live.recording.is_paused(),
                    !live.playing_along,
                )),
            ),
        };
        let stage = self.stage_rect;
        egui::Area::new(egui::Id::unique("record_bar"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(stage.center().x, stage.bottom() - BAR_LIFT))
            .constrain(true)
            .show(ctx, |ui| {
                // While recording it's one slim row, so it covers as little of the video as it can.
                if let Some((seconds, paused, can_pause)) = recording {
                    dialog_frame(ctx)
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                if can_pause && pause_button(ui, palette, paused) {
                                    asked = Asked::Pause(!paused);
                                }
                                if stop_button(ui, palette) {
                                    asked = Asked::Stop;
                                }
                                time_label(ui, palette, seconds);
                                ui.allocate_ui(vec2(SLIM_METER_WIDTH, METER_HEIGHT), |ui| {
                                    level_meter(ui, palette, level);
                                });
                                ui.add_space(4.0);
                            });
                        });
                    return;
                }
                dialog_frame(ctx).show(ui, |ui| {
                    ui.set_width(RECORD_BAR_WIDTH);
                    components::dialog_title(ui, &icons::MIC, palette.accent, "Record microphone");
                    ui.add_space(12.0);
                    if let Some(problem) = &problem {
                        ui.colored_label(palette.danger, problem);
                        ui.add_space(10.0);
                    }
                    match counting {
                        Some(left) => {
                            ui.vertical_centered(|ui| {
                                ui.colored_label(palette.text_muted, "Recording starts in");
                                ui.label(
                                    egui::RichText::new(left.to_string())
                                        .font(fonts::strong(40.0))
                                        .color(palette.text),
                                );
                            });
                        }
                        None => {
                            let shown = shown_microphone.clone();
                            let name = format!("Microphone: {shown}");
                            let width = ui.available_width();
                            let button =
                                ui.add(components::Dropdown::new(palette, &name, &shown, width));
                            egui::Popup::menu(&button).show(|ui| {
                                for microphone in &microphones {
                                    let item =
                                        MenuItem::choice(palette, microphone, *microphone == shown);
                                    if ui.add(item).clicked() {
                                        asked = Asked::Microphone(microphone.clone());
                                    }
                                }
                            });
                        }
                    }
                    ui.add_space(12.0);
                    level_meter(ui, palette, level);
                    ui.add_space(12.0);
                    if counting.is_none() {
                        let row =
                            SwitchRow::new(palette, "Play the video while recording", play_along);
                        if ui.add(row).clicked() {
                            asked = Asked::PlayAlong(!play_along);
                        }
                        let row = SwitchRow::new(palette, "Count down from 3", countdown);
                        if ui.add(row).clicked() {
                            asked = Asked::Countdown(!countdown);
                        }
                        ui.add_space(6.0);
                        if play_along {
                            ui.colored_label(
                                palette.text_muted,
                                "Wear headphones, or the microphone will hear the video too.",
                            );
                        }
                        ui.add_space(6.0);
                        if save_in_row(ui, palette, &folder) {
                            asked = Asked::Browse;
                        }
                        ui.add_space(12.0);
                    }
                    components::dialog_buttons(ui, |ui| {
                        if counting.is_none() {
                            let record =
                                ActionButton::new(palette, Kind::Primary, "Start recording")
                                    .icon(icons::CIRCLE_DOT)
                                    .width(DIALOG_BUTTON_WIDTH * 1.5);
                            if ui.add_enabled(problem.is_none(), record).clicked() {
                                asked = Asked::Record;
                            }
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
                if let Some(bar) = &mut self.record_bar {
                    // Without a countdown, it ends straight away.
                    let wait = if self.settings.countdown {
                        COUNTDOWN_SECONDS
                    } else {
                        0.0
                    };
                    bar.stage = BarStage::Counting(now + wait);
                    self.advance_record_bar(now);
                }
            }
            Asked::Cancel => self.record_bar = None,
            Asked::Stop => self.stop_recording(ctx),
            Asked::Pause(paused) => {
                if let Some(RecordBar {
                    stage: BarStage::Recording(live),
                    ..
                }) = &self.record_bar
                {
                    live.recording.set_paused(paused);
                }
            }
            Asked::Microphone(name) => self.choose_microphone(name),
            Asked::Countdown(on) => {
                self.settings.countdown = on;
                self.save_settings();
            }
            Asked::PlayAlong(on) => {
                self.settings.play_while_recording = on;
                self.save_settings();
            }
            Asked::Browse => {
                let from = self
                    .record_bar
                    .as_ref()
                    .and_then(|bar| bar.folder.clone())
                    .unwrap_or_else(|| self.recordings_folder());
                if let Some(picked) = (self.pick_folder)(&from)
                    && let Some(bar) = &mut self.record_bar
                {
                    bar.folder = Some(picked);
                }
            }
        }
    }

    /// Offers to add a recording that a run didn't stop, one at a time.
    pub(super) fn cut_short_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        let Some(CutShort { marker, file }) = self.cut_short.first().cloned() else {
            return;
        };
        let name = file
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let mut add = false;
        let mut leave = false;
        let modal = egui::Modal::new(egui::Id::unique("cut_short_dialog"))
            .frame(dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(RECORD_BAR_WIDTH);
                components::dialog_title(
                    ui,
                    &icons::MIC,
                    palette.accent,
                    "A recording was cut short",
                );
                ui.add_space(10.0);
                ui.label(format!(
                    "BareClip closed while {name} was recording. What was recorded until then is saved in {}.",
                    file.parent().map_or_else(String::new, |folder| folder.display().to_string())
                ));
                ui.add_space(16.0);
                components::dialog_buttons(ui, |ui| {
                    let button = ActionButton::new(palette, Kind::Primary, "Add to project")
                        .width(DIALOG_BUTTON_WIDTH * 1.4);
                    add = ui.add(button).clicked();
                    leave = ui.add(dialog_button(palette, "Not now")).clicked();
                });
            });
        if add || leave || modal.should_close() {
            storage::discard_recovery(&marker);
            self.cut_short.remove(0);
        }
        if add {
            let at = self.position();
            self.add_recording(ctx, &file, at);
        }
    }

    /// The Recording part of Settings.
    pub(super) fn recording_settings(&mut self, ui: &mut egui::Ui, palette: &Palette) -> bool {
        let mut changed = false;
        let folder = self.recordings_folder();
        components::settings_group(ui, palette, "Recording", |ui| {
            components::settings_row(
                ui,
                palette,
                ("Save recordings in", None),
                SETTINGS_CHOICE_WIDTH,
                |ui| {
                    let button = ActionButton::new(palette, Kind::Ghost, "Change")
                        .small()
                        .width(SETTINGS_CHOICE_WIDTH);
                    if ui.add(button).clicked()
                        && let Some(picked) = (self.pick_folder)(&folder)
                    {
                        self.settings.recordings_folder = Some(picked);
                        changed = true;
                    }
                },
            );
            // A long path wraps under its name rather than running under the button.
            let room = Margin {
                left: 12,
                right: 12,
                top: 0,
                bottom: 0,
            };
            Frame::new().inner_margin(room).show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(folder.display().to_string())
                            .size(12.0)
                            .color(palette.text_muted),
                    )
                    .wrap(),
                );
            });
            ui.add_space(8.0);
            components::settings_divider(ui, palette);
            let along = self.settings.play_while_recording;
            if ui
                .add(SwitchRow::new(
                    palette,
                    "Play the video while recording",
                    along,
                ))
                .clicked()
            {
                self.settings.play_while_recording = !along;
                changed = true;
            }
            components::settings_divider(ui, palette);
            let on = self.settings.countdown;
            if ui
                .add(SwitchRow::new(palette, "Count down from 3", on))
                .clicked()
            {
                self.settings.countdown = !on;
                changed = true;
            }
        });
        changed
    }
}

/// How loud the microphone is, as a bar that fills from the left.
fn level_meter(ui: &mut egui::Ui, palette: &Palette, fill: f32) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), METER_HEIGHT), Sense::hover());
    let percent = (fill * 100.0).round();
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::ProgressIndicator,
            true,
            format!("Microphone level {percent}%"),
        )
    });
    let radius = METER_HEIGHT / 2.0;
    ui.painter().rect_filled(rect, radius, palette.border);
    if fill > 0.0 {
        let filled = egui::Rect::from_min_size(rect.min, vec2(rect.width() * fill, rect.height()));
        ui.painter().rect_filled(filled, radius, palette.accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_running_time_shows_whole_seconds() {
        assert_eq!(recording_time(0.0), "0:00");
        assert_eq!(recording_time(7.99), "0:07");
        assert_eq!(recording_time(62.0), "1:02");
        assert_eq!(recording_time(599.5), "9:59");
        assert_eq!(recording_time(3727.0), "1:02:07");
        assert_eq!(recording_time(-1.0), "0:00");
    }

    #[test]
    fn the_meter_reads_loudness_in_decibels() {
        assert_eq!(meter_fill(0.0), 0.0);
        assert_eq!(meter_fill(1.0), 1.0);
        assert!((meter_fill(0.1) - 2.0 / 3.0).abs() < 0.001, "-20 dB");
        assert_eq!(meter_fill(0.0005), 0.0, "under -60 dB");
    }

    #[test]
    fn a_recording_is_named_by_its_kind_and_when() {
        assert_eq!(
            recording_name("Microphone", (2026, 11, 1, 9, 5, 3), "m4a"),
            "Microphone 2026-11-01 09-05-03.m4a"
        );
    }

    #[test]
    fn a_name_already_taken_gets_a_number() {
        let folder = std::env::temp_dir()
            .join("bareclip-recording-names")
            .join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("the folder can be made");
        assert_eq!(unused_path(&folder, "a.m4a"), folder.join("a.m4a"));
        std::fs::write(folder.join("a.m4a"), "").expect("written");
        assert_eq!(unused_path(&folder, "a.m4a"), folder.join("a (2).m4a"));
        std::fs::write(folder.join("a (2).m4a"), "").expect("written");
        assert_eq!(unused_path(&folder, "a.m4a"), folder.join("a (3).m4a"));
    }

    #[test]
    fn the_videos_folder_is_found() {
        let folder = default_recordings_folder();
        assert!(folder.ends_with(FOLDER_NAME));
        assert!(folder.parent().is_some_and(Path::is_absolute), "{folder:?}");
    }
}
