//! Recording the screen or a window, with the PC's sound and the microphone on tracks of their
//! own. The bar floats over the picture like the microphone's, with a small live picture of what
//! will be recorded. While recording, BareClip's windows keep themselves out of the picture, a
//! small overlay stays on top of everything with Pause, Stop and the microphone, and keys pause
//! and stop from any program. The overlay can be dragged by its grip and folded down to Pause and
//! Stop.

use std::sync::mpsc;

use bareclip_capture::{
    Offered, PictureSource, ScreenRecording, ScreenSettings, SoundFeed, Source,
};
use bareclip_media::Quality;

use super::recording::{
    MicSource, TIME_WIDTH, local_time, meter, meter_fill, pause_button, recording_name,
    save_in_row, stop_button, time_label, unused_path,
};
use super::*;
use crate::{
    hotkeys::{self, PAUSE_KEY, RecordingKey, RecordingKeys, STOP_KEY},
    storage::RecordingMarker,
};

const SCREEN_BAR_WIDTH: f32 = 420.0;
const PREVIEW_HEIGHT: f32 = 150.0;
/// How often the small picture of what will be recorded is redrawn, in seconds.
const PREVIEW_EVERY: f64 = 0.2;
const PREVIEW_WIDTH: u32 = 320;
const COUNTDOWN_SECONDS: f64 = 3.0;
const BAR_LIFT: f32 = 16.0;
/// How far in from the screen's corner the overlay sits.
const OVERLAY_MARGIN: f32 = 24.0;
/// The room around the overlay's controls.
pub(super) const OVERLAY_PADDING: i8 = 8;
/// The room between the recording controls.
const CONTROL_SPACING: f32 = 4.0;
const CONTROL_SIZE: f32 = 32.0;
const GRIP_WIDTH: f32 = 16.0;
const FOLD_WIDTH: f32 = 24.0;

/// Starts capturing a screen or window, with or without the pointer.
pub(super) type OpenScreen = fn(Source, bool) -> Result<Box<dyn PictureSource>, String>;
/// Starts listening to the PC's sound.
pub(super) type OpenPcSound = fn() -> Result<MicSource, String>;
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

pub(super) struct LiveScreen {
    recording: ScreenRecording,
    path: PathBuf,
    marker: Option<RecordingMarker>,
    lands_at: Ticks,
    keys: RecordingKeys,
    pressed: mpsc::Receiver<RecordingKey>,
    /// The microphone's track in the file. There is one whenever a microphone could be opened,
    /// even if it starts silenced, so it can be turned on midway.
    microphone_track: Option<usize>,
    /// The overlay is folded down to Pause and Stop. Every recording starts unfolded.
    folded: bool,
    /// Where the overlay first appears. It stays put from then on, until dragged.
    overlay_at: egui::Pos2,
}

impl LiveScreen {
    /// Whether the microphone is silenced, or `None` without a microphone track.
    fn microphone_muted(&self) -> Option<bool> {
        self.microphone_track
            .map(|track| self.recording.is_muted(track))
    }
}

/// What the recording controls show.
#[derive(Clone, Copy)]
struct Shown {
    seconds: f64,
    paused: bool,
    microphone_muted: Option<bool>,
    folded: bool,
}

/// The overlay's size: the grip, the controls and the fold button, with room around them.
pub(super) fn overlay_size(folded: bool) -> egui::Vec2 {
    let controls = if folded {
        [GRIP_WIDTH, CONTROL_SIZE, CONTROL_SIZE, FOLD_WIDTH].as_slice()
    } else {
        [
            GRIP_WIDTH,
            CONTROL_SIZE,
            CONTROL_SIZE,
            TIME_WIDTH,
            CONTROL_SIZE,
            FOLD_WIDTH,
        ]
        .as_slice()
    };
    let gaps = CONTROL_SPACING * (controls.len() - 1) as f32;
    let padding = 2.0 * f32::from(OVERLAY_PADDING);
    vec2(
        controls.iter().sum::<f32>() + gaps + padding,
        CONTROL_SIZE + padding,
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
    microphone: Option<Result<MicSource, String>>,
    pc_level: f32,
    microphone_level: f32,
    /// A folder picked with Browse for this recording only.
    pub(super) folder: Option<PathBuf>,
    pub(super) stage: ScreenStage,
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

impl Editor {
    pub(super) fn open_screen_bar(&mut self) {
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
        let pc_sound = self
            .settings
            .screen_pc_sound
            .then(|| (self.open_pc_sound)());
        let microphone = self
            .settings
            .screen_microphone
            .then(|| (self.open_microphone)(self.settings.microphone.as_deref()));
        self.screen_bar = Some(ScreenBar {
            offered,
            chosen,
            capture,
            preview: None,
            preview_count: 0,
            preview_at: 0.0,
            pc_sound,
            microphone,
            pc_level: 0.0,
            microphone_level: 0.0,
            folder: None,
            stage: ScreenStage::Waiting,
        });
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
    }

    fn start_screen_recording(&mut self, ctx: &egui::Context) {
        let default_folder = self.recordings_folder();
        let lands_at = self.position();
        let open_microphone = self.open_microphone;
        let microphone_name = self.settings.microphone.clone();
        let microphone_on = self.settings.screen_microphone;
        let monitor = ctx
            .input(|input| input.viewport().monitor_size)
            .unwrap_or(vec2(1920.0, 1080.0));
        let overlay = overlay_size(false);
        let overlay_at = pos2(
            monitor.x - overlay.x - OVERLAY_MARGIN,
            monitor.y - overlay.y - OVERLAY_MARGIN * 3.0,
        );
        let settings = ScreenSettings {
            frame_rate: self.settings.screen_frame_rate,
            size: None,
            quality: Quality::Balanced,
        };
        let Some(bar) = &mut self.screen_bar else {
            return;
        };
        let capture = std::mem::replace(&mut bar.capture, Err("Recording".to_owned()));
        let Ok(capture) = capture else {
            bar.capture = capture;
            bar.stage = ScreenStage::Waiting;
            return;
        };
        let folder = bar.folder.clone().unwrap_or(default_folder);
        // The microphone gets a track even when switched off, silenced, so it can be turned on
        // midway. A track can't be added once the file has begun.
        if bar.microphone.is_none() {
            bar.microphone = Some(open_microphone(microphone_name.as_deref()));
        }
        let mut sounds = Vec::new();
        let mut microphone_track = None;
        for (source, name, muted) in [
            (&mut bar.pc_sound, "PC sound", false),
            (&mut bar.microphone, "Microphone", !microphone_on),
        ] {
            if let Some(Ok(source)) = source
                && let Some(queue) = source.queue.take()
            {
                if name == "Microphone" {
                    microphone_track = Some(sounds.len());
                }
                sounds.push(SoundFeed {
                    queue,
                    rate: source.rate,
                    name: name.to_owned(),
                    muted,
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
        let path = unused_path(&folder, &recording_name("Screen", local_time(), "mp4"));
        // The keys are taken first, so the file starts only once they are ready.
        let (sender, pressed) = mpsc::channel();
        let wake = ctx.clone();
        let keys = RecordingKeys::listen(sender, move || wake.request_repaint());
        match ScreenRecording::start(&path, capture, sounds, settings) {
            Ok(recording) => {
                let marker = self
                    .app_folder
                    .as_ref()
                    .and_then(|app| RecordingMarker::start(app, &path).ok());
                hotkeys::hide_from_captures(true);
                if !keys.taken {
                    self.notice = Some(format!(
                        "Another program uses {} or {}, so pause and stop with the recording bar.",
                        STOP_KEY.1, PAUSE_KEY.1
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
                        folded: false,
                        overlay_at,
                    }));
                }
            }
            Err(error) => {
                self.notice = Some(format!("The recording could not start. {error}"));
                self.screen_bar = None;
            }
        }
    }

    /// Finishes the file and adds it to the project.
    pub(super) fn stop_screen_recording(&mut self, ctx: &egui::Context) {
        let Some(bar) = self.screen_bar.take() else {
            return;
        };
        let ScreenStage::Recording(live) = bar.stage else {
            return;
        };
        let LiveScreen {
            recording,
            path,
            marker,
            lands_at,
            keys,
            ..
        } = *live;
        drop(keys);
        hotkeys::hide_from_captures(false);
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

    fn set_screen_paused(&mut self, paused: bool) {
        if let Some(ScreenBar {
            stage: ScreenStage::Recording(live),
            ..
        }) = &self.screen_bar
        {
            live.recording.set_paused(paused);
        }
    }

    fn set_screen_microphone_muted(&mut self, muted: bool) {
        if let Some(ScreenBar {
            stage: ScreenStage::Recording(live),
            ..
        }) = &self.screen_bar
            && let Some(track) = live.microphone_track
        {
            live.recording.set_muted(track, muted);
        }
    }

    /// Keeps the small picture and the meters up to date.
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
        let mut keyed = None;
        if let Some(ScreenBar {
            stage: ScreenStage::Recording(live),
            ..
        }) = &self.screen_bar
        {
            while let Ok(key) = live.pressed.try_recv() {
                keyed = Some((key, live.recording.is_paused()));
            }
        }
        match keyed {
            Some((RecordingKey::Stop, _)) => {
                self.stop_screen_recording(ctx);
                return;
            }
            Some((RecordingKey::Pause, paused)) => self.set_screen_paused(!paused),
            None => {}
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
            Microphone(bool),
            FrameRate(u32),
            Pointer(bool),
            Countdown(bool),
            MicrophoneMuted(bool),
            Folded(bool),
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
        let folder = bar
            .folder
            .clone()
            .unwrap_or_else(|| self.recordings_folder());
        egui::Area::new(egui::Id::unique("screen_bar"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(stage.center().x, stage.bottom() - BAR_LIFT))
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
                        });
                    return;
                }
                dialog_frame(ctx).show(ui, |ui| {
                    ui.set_width(SCREEN_BAR_WIDTH);
                    components::dialog_title(ui, &icons::MONITOR, palette.accent, "Record screen");
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
                    // What will be recorded, as it looks now.
                    let width = ui.available_width();
                    let (spot, _) =
                        ui.allocate_exact_size(vec2(width, PREVIEW_HEIGHT), Sense::hover());
                    ui.painter().rect_filled(spot, 6.0, palette.stage);
                    match (&bar.capture, &bar.preview) {
                        (Ok(_), Some(texture)) => {
                            let size = texture.size_vec2();
                            let scale = (spot.width() / size.x).min(spot.height() / size.y);
                            let shown = egui::Rect::from_center_size(spot.center(), size * scale);
                            ui.painter().image(
                                texture.id(),
                                shown,
                                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
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
                    ui.add_space(10.0);
                    let shown = bar.chosen_name();
                    let name = format!("Record: {shown}");
                    let button = ui.add(components::Dropdown::new(palette, &name, &shown, width));
                    egui::Popup::menu(&button).show(|ui| {
                        egui::ScrollArea::vertical()
                            .max_height(320.0)
                            .show(ui, |ui| {
                                for offered in &bar.offered {
                                    let item = MenuItem::choice(
                                        palette,
                                        &offered.name,
                                        bar.chosen == Some(offered.source),
                                    );
                                    if ui.add(item).clicked() {
                                        asked = Asked::Source(offered.source);
                                    }
                                }
                            });
                    });
                    ui.add_space(8.0);
                    let sound_row = |ui: &mut egui::Ui,
                                     name: &str,
                                     on: bool,
                                     source: &Option<Result<MicSource, String>>,
                                     level: f32|
                     -> bool {
                        let clicked = ui.add(SwitchRow::new(palette, name, on)).clicked();
                        match source {
                            Some(Ok(_)) => meter(ui, palette, level),
                            Some(Err(problem)) => {
                                ui.colored_label(palette.danger, problem);
                            }
                            None => {}
                        }
                        clicked
                    };
                    if sound_row(
                        ui,
                        "PC sound",
                        settings.screen_pc_sound,
                        &bar.pc_sound,
                        bar.pc_level,
                    ) {
                        asked = Asked::PcSound(!settings.screen_pc_sound);
                    }
                    if sound_row(
                        ui,
                        "Microphone",
                        settings.screen_microphone,
                        &bar.microphone,
                        bar.microphone_level,
                    ) {
                        asked = Asked::Microphone(!settings.screen_microphone);
                    }
                    if ui
                        .add(SwitchRow::new(
                            palette,
                            "Show the mouse pointer",
                            settings.screen_pointer,
                        ))
                        .clicked()
                    {
                        asked = Asked::Pointer(!settings.screen_pointer);
                    }
                    if ui
                        .add(SwitchRow::new(
                            palette,
                            "Count down from 3",
                            settings.countdown,
                        ))
                        .clicked()
                    {
                        asked = Asked::Countdown(!settings.countdown);
                    }
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label("Frame rate");
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            for rate in [60, 30] {
                                let chosen = settings.screen_frame_rate == rate;
                                let label = format!("{rate} fps");
                                let button = ActionButton::new(palette, Kind::Ghost, &label)
                                    .small()
                                    .selected(chosen);
                                if ui.add(button).clicked() && !chosen {
                                    asked = Asked::FrameRate(rate);
                                }
                            }
                        });
                    });
                    ui.add_space(8.0);
                    if save_in_row(ui, palette, &folder) {
                        asked = Asked::Browse;
                    }
                    ui.add_space(4.0);
                    ui.colored_label(
                        palette.text_muted,
                        format!(
                            "{} stops and {} pauses from any program.",
                            STOP_KEY.1, PAUSE_KEY.1,
                        ),
                    );
                    ui.add_space(12.0);
                    components::dialog_buttons(ui, |ui| {
                        let record = ActionButton::new(palette, Kind::Primary, "Start recording")
                            .icon(icons::CIRCLE_DOT)
                            .width(DIALOG_BUTTON_WIDTH * 1.5);
                        if ui.add_enabled(bar.capture.is_ok(), record).clicked() {
                            asked = Asked::Record;
                        }
                        if ui.add(dialog_button(palette, "Cancel")).clicked() {
                            asked = Asked::Cancel;
                        }
                    });
                });
            });
        if let Some((shown, at)) = recording
            && let Some(choice) = recording_overlay(ctx, palette, shown, at)
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
            Asked::Pause(paused) => self.set_screen_paused(paused),
            Asked::Source(source) => self.choose_source(source),
            Asked::PcSound(on) => {
                self.settings.screen_pc_sound = on;
                let opened = on.then(|| (self.open_pc_sound)());
                if let Some(bar) = &mut self.screen_bar {
                    bar.pc_sound = opened;
                }
                save(self);
            }
            Asked::Microphone(on) => {
                self.settings.screen_microphone = on;
                let opened =
                    on.then(|| (self.open_microphone)(self.settings.microphone.as_deref()));
                if let Some(bar) = &mut self.screen_bar {
                    bar.microphone = opened;
                }
                save(self);
            }
            Asked::FrameRate(rate) => {
                self.settings.screen_frame_rate = rate;
                save(self);
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
            Asked::MicrophoneMuted(muted) => self.set_screen_microphone_muted(muted),
            Asked::Folded(folded) => {
                if let Some(ScreenBar {
                    stage: ScreenStage::Recording(live),
                    ..
                }) = &mut self.screen_bar
                {
                    live.folded = folded;
                }
            }
            Asked::Browse => {
                let pick = self.pick_folder;
                if let Some(bar) = &mut self.screen_bar
                    && let Some(picked) = pick(&folder)
                {
                    bar.folder = Some(picked);
                }
            }
        }

        /// Pause, Stop, the time and the microphone, in the bar and on the overlay alike.
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
            asked
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

        /// A small window on top of everything, starting near the screen's bottom right corner,
        /// so the recording can be paused and stopped from inside a game. It keeps itself out of
        /// the recording like BareClip's other windows. The whole window is painted, so no part
        /// of it is left black.
        fn recording_overlay(
            ctx: &egui::Context,
            palette: &Palette,
            shown: Shown,
            at: egui::Pos2,
        ) -> Option<Asked> {
            let builder = egui::ViewportBuilder::default()
                .with_title("BareClip recording")
                .with_inner_size(overlay_size(shown.folded))
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
                            ui.horizontal(|ui| {
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
                            .inner
                        })
                        .inner
                },
            )
        }
    }
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
}
