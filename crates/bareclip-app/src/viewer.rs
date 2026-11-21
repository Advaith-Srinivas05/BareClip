//! The player: what a double-click on a video or sound opens. It is a window of its own, with
//! the editor's title strip, the picture in the middle and a bar of controls along the bottom.
//!
//! It plays through the same `Preview` as the editor, on a private timeline holding one clip,
//! the whole file, so sound is the clock and seeking, frame steps and sound at other speeds all
//! come from code the editor already uses. Nothing about it is saved into the file, and its
//! speed only lasts while the window is open.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use bareclip_audio::{Bus, MixSpan};
use bareclip_core::{
    sound::{ClipSound, FULL_VOLUME},
    speed::Speed,
    splice::FrameIndex,
    time::{Ticks, format_timecode},
    timeline::{Clip, ClipId, MediaId, TimeRange},
    tracks::Tracks,
};
use bareclip_media::{MediaInfo, probe};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, Layout, Modifiers, Rect, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, UiBuilder, pos2, vec2,
};

use windows_sys::Win32::Foundation::HWND;
use zoom::Zoom;

use crate::{
    components::{self, ActionButton, IconButton, Kind, MenuItem},
    fonts, icons,
    preview::Preview,
    storage::{self, Settings, ThemeSetting, ViewerSettings, WindowPlace},
    text, theme,
    theme::Palette,
};

/// The smallest the player's window goes, which leaves room for mini view.
pub const SMALLEST_WINDOW: [f32; 2] = [320.0, 180.0];
/// The watching speeds, slowest first.
pub const SPEEDS: [f64; 7] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
/// Where 1x is in `SPEEDS`.
const NORMAL_SPEED: usize = 3;
/// How long the bars stay in full screen once the mouse stops moving.
const BARS_LINGER: Duration = Duration::from_secs(2);
/// How much Up and Down change the volume, out of 100.
const VOLUME_STEP: u32 = 5;
/// A step forward or back when the frame table isn't read yet, or there's no picture.
const FALLBACK_STEP_SECONDS: f64 = 1.0 / 30.0;

const TITLE_HEIGHT: f32 = 48.0;
const SEEK_ROW_HEIGHT: f32 = 26.0;
const CONTROL_ROW_HEIGHT: f32 = 44.0;
const BOTTOM_HEIGHT: f32 = SEEK_ROW_HEIGHT + CONTROL_ROW_HEIGHT;
const WINDOW_BUTTON_SIZE: f32 = 40.0;
const CONTROL_SPACING: f32 = 4.0;
const VOLUME_BAR_WIDTH: f32 = 80.0;
const TIME_WIDTH: f32 = 72.0;
/// How wide the controls in the middle of the bottom bar are.
const MIDDLE_WIDTH: f32 = 470.0;
/// How large the BareClip mark shows for a sound file with no cover.
const MARK_SIDE: f32 = 160.0;
/// How far one notch of a mouse wheel scrolls, in points.
const WHEEL_NOTCH: f32 = 50.0;
/// How long the note about a saved picture stays.
const SAVED_NOTE_FOR: Duration = Duration::from_secs(8);

/// The one clip on the player's timeline.
const MEDIA: MediaId = MediaId(1);

/// What a background read of the open file sends back.
enum Arrived {
    Frames(Result<FrameIndex, String>),
    Cover(Option<bareclip_core::picture::StillPicture>),
}

/// The file being shown, and the private timeline it plays on.
struct OpenFile {
    info: MediaInfo,
    tracks: Tracks,
    clip: ClipId,
    /// The file's frames, read in the background, for frame steps and the trimmer.
    frames: Option<FrameIndex>,
    /// A sound file's cover, once read, or `None` when it has none.
    cover: Option<TextureHandle>,
    cover_read: bool,
    arrivals: Option<Receiver<Arrived>>,
    /// Stops the background reads when another file opens.
    cancel: Arc<AtomicBool>,
    /// The thread reading the frames or the cover, which has the file open while it runs.
    reader: Option<thread::JoinHandle<()>>,
}

impl OpenFile {
    fn clip(&self) -> Clip {
        self.tracks
            .clip(self.clip)
            .expect("the player's one clip is always there")
    }

    fn is_sound(&self) -> bool {
        self.info.is_sound_only()
    }
}

impl Drop for OpenFile {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// A file the player let go of, and how to carry on with it.
pub(super) struct Released {
    pub path: PathBuf,
    pub at: Ticks,
    pub playing: bool,
}

/// A file that couldn't be opened, and why.
struct Unplayable {
    path: PathBuf,
    reason: String,
}

pub struct Viewer {
    preview: Preview,
    file: Option<OpenFile>,
    failed: Option<Unplayable>,
    /// The picture on screen.
    picture: Option<TextureHandle>,
    mark: Option<TextureHandle>,
    settings: ViewerSettings,
    theme: ThemeSetting,
    app_folder: Option<PathBuf>,
    /// Which of `SPEEDS` is playing.
    speed: usize,
    looping: bool,
    full_screen: bool,
    last_pointer_move: Instant,
    last_pointer: Option<egui::Pos2>,
    seek_bar_held: bool,
    resume_after_scrub: bool,
    volume_held: bool,
    settings_open: bool,
    /// Something to tell the user, shown over the bottom of the picture.
    notice: Option<String>,
    /// Was playing last frame, to notice playback reaching the end.
    was_playing: bool,
    /// How far into the picture it is zoomed.
    zoom: Zoom,
    /// Where the picture was shown last frame, which zoom works in.
    view: Rect,
    /// A drag on the picture that pans it.
    panning: bool,
    /// The window as Windows knows it, for putting it back after mini view.
    window: Option<HWND>,
    mini: Option<mini::Mini>,
    /// A picture being saved in the background.
    saving_picture: Option<Receiver<Result<PathBuf, String>>>,
    /// The picture last saved, which a note offers to show in its folder, and since when.
    saved_picture: Option<(PathBuf, Instant)>,
    /// The playable files in the open file's folder, in name order.
    folder: Vec<PathBuf>,
    folder_reading: Option<Receiver<Vec<PathBuf>>>,
    /// Where files were last stopped.
    resume: resume::ResumeSpots,
    /// When the playing file's spot was last written to disk.
    spot_kept: Instant,
    /// Where the open file picked up, and since when, for Start over.
    resumed: Option<(Ticks, Instant)>,
    /// The info panel, while it is open. It stays open from file to file.
    info: Option<info::InfoPanel>,
    /// Delete is waiting for an answer.
    confirm_delete: bool,
    /// Moves a file to the Recycle Bin. Tests swap it for a folder of their own.
    trash_file: fn(&Path) -> Result<(), String>,
    /// The quick trimmer, while it is open.
    trim: Option<trim::Trimmer>,
    /// Where Edit in BareClip looks for editors. Tests use names of their own.
    editor_prefix: String,
    /// A clip to open in an editor this window becomes, because none was open.
    become_editor: Option<crate::handoff::Handed>,
}

impl Viewer {
    pub fn new(ctx: &egui::Context, use_sound: bool) -> Self {
        let repaint = ctx.clone();
        Self {
            preview: Preview::new(use_sound, Arc::new(move || repaint.request_repaint())),
            file: None,
            failed: None,
            picture: None,
            mark: None,
            settings: ViewerSettings::default(),
            theme: ThemeSetting::System,
            app_folder: None,
            speed: NORMAL_SPEED,
            looping: false,
            full_screen: false,
            last_pointer_move: Instant::now(),
            last_pointer: None,
            seek_bar_held: false,
            resume_after_scrub: false,
            volume_held: false,
            settings_open: false,
            notice: None,
            was_playing: false,
            zoom: Zoom::fit(),
            view: Rect::NOTHING,
            panning: false,
            window: None,
            mini: None,
            saving_picture: None,
            saved_picture: None,
            folder: Vec::new(),
            folder_reading: None,
            resume: resume::ResumeSpots::default(),
            spot_kept: Instant::now(),
            resumed: None,
            info: None,
            confirm_delete: false,
            trash_file: crate::file_ops::move_to_recycle_bin,
            trim: None,
            editor_prefix: crate::handoff::PIPE_PREFIX.to_owned(),
            become_editor: None,
        }
    }

    /// Tells the player which window it is in, so mini view can put it back where it was.
    pub fn use_window(&mut self, window: Option<HWND>) {
        self.window = window;
    }

    /// Reads the player's settings, and the theme it shares with the editor.
    pub fn use_storage(&mut self, ctx: &egui::Context, folder: PathBuf) {
        let settings = Settings::load(&folder);
        self.settings = settings.viewer;
        self.theme = settings.theme;
        ctx.set_theme(egui::ThemePreference::from(self.theme));
        self.resume = resume::ResumeSpots::load(&folder);
        self.app_folder = Some(folder);
    }

    /// Where the player was when it last closed.
    pub fn saved_window(&self) -> Option<WindowPlace> {
        self.settings.window
    }

    /// Keeps where the window is as it closes, for the next run.
    pub fn closing(&mut self, place: Option<WindowPlace>) {
        if let Some(place) = place {
            self.settings.window = Some(place);
        }
        self.save_settings();
        self.remember_spot();
        self.save_spots();
    }

    fn save_settings(&mut self) {
        if let Some(folder) = &self.app_folder
            && let Err(error) = Settings::save_from_viewer(folder, &self.settings, self.theme)
        {
            self.notice = Some(format!("Your settings could not be saved. {error}"));
        }
    }

    /// Opens what the app was started with. Explorer gives one file a launch, so only the first
    /// opens.
    pub fn open_launch(&mut self, ctx: &egui::Context, paths: &[PathBuf]) {
        if let Some(path) = paths.first() {
            self.open(ctx, path);
        }
    }

    /// Shows a file, starting it playing from the start.
    pub fn open(&mut self, ctx: &egui::Context, path: &Path) {
        self.close_file();
        self.trim = None;
        self.zoom = Zoom::fit();
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        match probe(&path) {
            Ok(info) if !info.is_still() => {
                let file = self.start_file(ctx, info);
                self.file = Some(file);
                self.sync_sound();
                self.resume_if_stopped_partway();
                self.play();
            }
            Ok(_) => {
                self.failed = Some(Unplayable {
                    path,
                    reason: "This is a picture, not a video or sound.".to_owned(),
                });
            }
            Err(error) => {
                self.failed = Some(Unplayable {
                    path,
                    reason: text::unplayable_reason(&error),
                });
            }
        }
        self.read_folder(ctx);
        ctx.request_repaint();
    }

    /// Builds the private timeline for a file and starts its background reads.
    fn start_file(&mut self, ctx: &egui::Context, info: MediaInfo) -> OpenFile {
        let mut tracks = Tracks::default();
        let whole = TimeRange {
            start: Ticks(0),
            end: Ticks(info.duration.0.max(1)),
        };
        let clip = if info.is_sound_only() {
            tracks.push_sound(MEDIA, whole)
        } else {
            tracks.push(MEDIA, whole)
        };
        // The watching speed carries on from file to file while the window is open.
        tracks.set_speed(clip, watching_speed(self.speed));
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, arrivals) = mpsc::channel();
        let path = info.path.clone();
        let sound = info.is_sound_only();
        let stop = cancel.clone();
        let repaint = ctx.clone();
        let reader = thread::spawn(move || {
            if sound {
                let cover = bareclip_media::read_cover(&path).ok().flatten();
                let _ = sender.send(Arrived::Cover(cover));
            } else {
                let frames = bareclip_media::scan_frames(&path, &stop, |_| {})
                    .map_err(|error| error.to_string());
                let _ = sender.send(Arrived::Frames(frames));
            }
            repaint.request_repaint();
        });
        OpenFile {
            info,
            tracks,
            clip,
            frames: None,
            cover: None,
            cover_read: false,
            arrivals: Some(arrivals),
            cancel,
            reader: Some(reader),
        }
    }

    /// Lets go of the open file, stopping what plays and reads it.
    fn close_file(&mut self) {
        if self.file.is_some() {
            self.remember_spot();
            self.save_spots();
        }
        self.resumed = None;
        if let Some(file) = &self.file {
            self.preview.pause(file.tracks.played_videos());
        }
        self.preview.close(MEDIA);
        self.preview.set_sound(Vec::new(), Ticks(0));
        self.file = None;
        self.failed = None;
        self.picture = None;
        self.notice = None;
        self.was_playing = false;
    }

    /// Lets go of the file entirely, so Windows will rename, change or delete it: the pictures,
    /// the sound and every background read stop, and this waits for the readers to finish. The
    /// last picture stays on screen. Gives what `reopen` needs to carry on.
    pub(super) fn release_file(&mut self) -> Option<Released> {
        let released = Released {
            path: self.file.as_ref()?.info.path.clone(),
            at: self.position(),
            playing: self.is_playing(),
        };
        self.remember_spot();
        self.save_spots();
        self.pause();
        self.preview.close(MEDIA);
        self.preview.set_sound(Vec::new(), Ticks(0));
        if let Some(mut file) = self.file.take() {
            file.cancel.store(true, Ordering::Relaxed);
            if let Some(reader) = file.reader.take() {
                let _ = reader.join();
            }
        }
        if let Some(info) = &mut self.info {
            info.stop_reading();
        }
        self.resumed = None;
        self.was_playing = false;
        Some(released)
    }

    /// Opens a file again after it was let go, at the same moment, playing or paused as it was,
    /// keeping the zoom. It may have a new name.
    pub(super) fn reopen(&mut self, ctx: &egui::Context, path: &Path, at: Ticks, playing: bool) {
        let zoom = self.zoom;
        let resume = std::mem::replace(&mut self.settings.resume, false);
        self.open(ctx, path);
        self.settings.resume = resume;
        self.zoom = zoom;
        self.pause();
        self.seek(at);
        if playing {
            self.play();
        }
    }

    /// What can be heard, at the player's volume and speed.
    fn sync_sound(&mut self) {
        let spans = self.heard();
        if let Some(file) = &self.file {
            self.preview.set_sound(spans, file.tracks.duration());
        }
    }

    /// What can be heard: the whole file, every track of it together, as the editor plays it.
    fn heard(&self) -> Vec<MixSpan> {
        let Some(file) = &self.file else {
            return Vec::new();
        };
        if file.info.audio.is_empty() {
            Vec::new()
        } else {
            let clip = file.clip();
            vec![MixSpan {
                path: file.info.path.clone(),
                start: clip.start,
                source: clip.source,
                sound: ClipSound {
                    volume: (f64::from(FULL_VOLUME) * f64::from(self.settings.loudness())).round()
                        as u32,
                    ..ClipSound::default()
                },
                speed: clip.speed,
                bus: Bus::All(0),
            }]
        }
    }

    fn lookup(info: &MediaInfo) -> impl Fn(MediaId) -> Option<MediaInfo> + '_ {
        move |id| (id == MEDIA).then(|| info.clone())
    }

    pub fn is_playing(&self) -> bool {
        self.preview.is_playing()
    }

    fn play(&mut self) {
        if let Some(file) = &self.file {
            self.preview
                .play(file.tracks.played_videos(), &Self::lookup(&file.info));
        }
    }

    fn pause(&mut self) {
        if let Some(file) = &self.file {
            self.preview.pause(file.tracks.played_videos());
        }
    }

    pub fn toggle_playback(&mut self) {
        if self.is_playing() {
            self.pause();
        } else if self.trim.is_some() {
            self.play_in_trim();
        } else {
            self.play();
        }
    }

    /// Where the playhead is in the file, which differs from the timeline's time at other speeds.
    pub fn position(&self) -> Ticks {
        let Some(file) = &self.file else {
            return Ticks(0);
        };
        let clip = file.clip();
        clip.source_at(self.preview.position(file.tracks.played_videos()))
            .clamp(clip.source.start, clip.source.end)
    }

    /// How long the open file is.
    pub fn duration(&self) -> Ticks {
        self.file
            .as_ref()
            .map_or(Ticks(0), |file| file.info.duration)
    }

    /// Moves the playhead to a moment in the file.
    pub fn seek(&mut self, time: Ticks) {
        let Some(file) = &self.file else {
            return;
        };
        let clip = file.clip();
        let time = time.clamp(clip.source.start, clip.source.end);
        self.preview.seek(
            file.tracks.played_videos(),
            &Self::lookup(&file.info),
            clip.time_of(time),
        );
    }

    /// Jumps by the seek step from Settings.
    pub fn skip(&mut self, forward: bool) {
        let step = Ticks::from_seconds(f64::from(self.settings.seek_step()));
        let target = if forward {
            Ticks(self.position().0 + step.0)
        } else {
            Ticks(self.position().0 - step.0)
        };
        self.seek(target);
    }

    /// Pauses and moves one frame on or back, by the file's own frames once they are read.
    pub fn step_frame(&mut self, forward: bool) {
        self.pause();
        let Some(file) = &self.file else {
            return;
        };
        let position = self.position();
        let fallback = Ticks::from_seconds(if file.info.video.frame_rate > 0.0 {
            1.0 / file.info.video.frame_rate
        } else {
            FALLBACK_STEP_SECONDS
        });
        let stepped = file.frames.as_ref().and_then(|frames| {
            let base = file.info.video.time_base;
            let shown = frames.frame_at(position.to_pts(base))?;
            let neighbour = if forward {
                frames.next_frame_after(shown)?
            } else {
                frames.previous_frame_before(shown)?
            };
            Some(Ticks::from_pts(neighbour, base))
        });
        let target = stepped.unwrap_or(if forward {
            Ticks(position.0 + fallback.0)
        } else {
            Ticks(position.0 - fallback.0)
        });
        self.seek(target);
    }

    /// The watching speed, as a factor.
    pub fn speed(&self) -> f64 {
        SPEEDS[self.speed]
    }

    /// Changes how fast it plays, keeping the voice's pitch. It only lasts while the window is
    /// open and never reaches the file.
    pub fn set_speed(&mut self, index: usize) {
        let index = index.min(SPEEDS.len() - 1);
        let at = self.position();
        let playing = self.is_playing();
        self.speed = index;
        if let Some(file) = &mut self.file {
            file.tracks.set_speed(file.clip, watching_speed(index));
        }
        self.sync_sound();
        if playing {
            self.pause();
            self.seek(at);
            self.play();
        } else {
            self.seek(at);
        }
    }

    pub fn volume(&self) -> u32 {
        self.settings.volume.min(100)
    }

    pub fn is_muted(&self) -> bool {
        self.settings.muted
    }

    pub fn set_volume(&mut self, volume: u32) {
        self.settings.volume = volume.min(100);
        self.settings.muted = false;
        self.sync_sound();
    }

    pub fn toggle_mute(&mut self) {
        self.settings.muted = !self.settings.muted;
        self.sync_sound();
    }

    pub fn seek_step(&self) -> u32 {
        self.settings.seek_step()
    }

    pub fn set_seek_step(&mut self, seconds: u32) {
        self.settings.seek_seconds = seconds;
        self.settings.seek_seconds = self.settings.seek_step();
        self.save_settings();
    }

    #[cfg(test)]
    pub fn is_looping(&self) -> bool {
        self.looping
    }

    #[cfg(test)]
    pub fn is_full_screen(&self) -> bool {
        self.full_screen
    }

    fn set_full_screen(&mut self, ctx: &egui::Context, full: bool) {
        if self.full_screen != full {
            self.full_screen = full;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(full));
            self.last_pointer_move = Instant::now();
        }
    }

    /// The file's name, for the title strip.
    fn name(&self) -> String {
        let path = self
            .file
            .as_ref()
            .map(|file| file.info.path.as_path())
            .or(self.failed.as_ref().map(|failed| failed.path.as_path()));
        path.and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Draws the whole window.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let palette = theme::palette(&ctx);
        self.take_arrivals(&ctx);
        self.handle_keys(&ctx);
        self.follow_pointer(&ctx);
        self.update_picture(&ctx);
        self.hold_playback_while_scrubbing();

        self.take_saved_picture(&ctx);
        self.take_folder();
        self.keep_spot_while_playing();
        self.poll_trimmer(&ctx);
        self.keep_within_trim();

        let whole = ui.max_rect();
        if self.mini.is_some() {
            // Mini view is only the picture, with its controls on hover.
            self.video_area(ui, palette, whole);
            self.mini_controls(ui, palette, whole);
            self.show_notice(ui, palette, whole);
            return;
        }
        let bars = self.bars_shown(&ctx);
        let title = Rect::from_min_size(whole.min, vec2(whole.width(), TITLE_HEIGHT));
        let bottom_height = if self.trim.is_some() {
            trim::TRIM_ROW_HEIGHT + trim::ACTION_ROW_HEIGHT + CONTROL_ROW_HEIGHT
        } else {
            BOTTOM_HEIGHT
        };
        let bottom = Rect::from_min_max(
            pos2(whole.left(), whole.bottom() - bottom_height),
            whole.max,
        );
        let mut video = if self.full_screen {
            whole
        } else {
            Rect::from_min_max(
                pos2(whole.left(), title.bottom()),
                pos2(whole.right(), bottom.top()),
            )
        };
        // The info panel sits beside the picture, as long as there is room for both.
        let panel =
            (self.info.is_some() && !self.full_screen && video.width() > info::PANEL_WIDTH * 2.0)
                .then(|| {
                    let panel = Rect::from_min_max(
                        pos2(video.right() - info::PANEL_WIDTH, video.top()),
                        video.max,
                    );
                    video.max.x = panel.left();
                    panel
                });
        self.video_area(ui, palette, video);
        self.edge_arrows(ui, palette, video);
        self.start_over_note(ui, palette, video);
        if let Some(panel) = panel {
            self.info_panel(ui, palette, panel);
        }
        if bars {
            self.title_strip(ui, palette, title);
            self.bottom_bar(ui, palette, bottom);
        }
        self.settings_window(&ctx, palette);
        self.delete_dialog(&ctx, palette);
        self.mp4_question(&ctx, palette);
        self.show_notice(ui, palette, video);
        self.saved_picture_note(ui, palette, video);
        crate::editor::window_edges(&ctx);
    }

    /// The video's size in pixels, for a file with a picture.
    fn video_size(&self) -> Option<egui::Vec2> {
        let file = self.file.as_ref().filter(|file| !file.is_sound())?;
        Some(vec2(
            file.info.video.width as f32,
            file.info.video.height as f32,
        ))
    }

    /// Zooms by a factor toward a point, or the middle of the picture.
    pub fn zoom_by(&mut self, ctx: &egui::Context, factor: f32, toward: Option<egui::Pos2>) {
        let Some(video) = self.video_size() else {
            return;
        };
        let view = self.view;
        if view.width() <= 0.0 {
            return;
        }
        let toward = toward.unwrap_or(view.center());
        self.zoom = self
            .zoom
            .zoom_by(view, video, factor, toward, ctx.pixels_per_point());
    }

    /// Back to the whole picture.
    pub fn fit(&mut self) {
        self.zoom = Zoom::fit();
    }

    /// The zoom as a percentage of the video's real pixels.
    pub fn zoom_percent(&self, ctx: &egui::Context) -> f32 {
        self.video_size().map_or(100.0, |video| {
            self.zoom.percent(self.view, video, ctx.pixels_per_point())
        })
    }

    /// Decodes the video at full size while it is shown larger than the preview's pictures, so
    /// it stays sharp, and goes back to the smaller pictures at fit.
    fn keep_sharp(&mut self, ctx: &egui::Context) {
        let Some(video) = self.video_size() else {
            return;
        };
        let (width, height) = bareclip_media::fit_preview_size(
            video.x as u32,
            video.y as u32,
            crate::player::PREVIEW_WIDTH,
            crate::player::PREVIEW_HEIGHT,
        );
        let sharp = self.zoom.wants_sharp(
            self.view,
            video,
            vec2(width as f32, height as f32),
            ctx.pixels_per_point(),
        );
        let Some(file) = &self.file else {
            return;
        };
        self.preview.set_sharp(
            if sharp { vec![MEDIA] } else { Vec::new() },
            file.tracks.played_videos(),
            &Self::lookup(&file.info),
        );
    }

    #[cfg(test)]
    pub fn is_sharp(&self) -> bool {
        self.preview.is_sharp(MEDIA)
    }

    /// Takes in what the background reads found.
    fn take_arrivals(&mut self, ctx: &egui::Context) {
        let Some(file) = &mut self.file else {
            return;
        };
        let Some(arrivals) = &file.arrivals else {
            return;
        };
        let mut finished = false;
        while let Ok(arrived) = arrivals.try_recv() {
            match arrived {
                Arrived::Frames(Ok(frames)) => file.frames = Some(frames),
                // Playing doesn't need the frame table, so a file that can't be scanned still
                // plays, stepping by its frame rate.
                Arrived::Frames(Err(_)) => {}
                Arrived::Cover(cover) => {
                    file.cover_read = true;
                    file.cover = cover.map(|cover| {
                        let image = egui::ColorImage::from_rgba_premultiplied(
                            [cover.width as usize, cover.height as usize],
                            &cover.rgba,
                        );
                        ctx.load_texture("viewer-cover", image, TextureOptions::LINEAR)
                    });
                }
            }
            finished = true;
        }
        if finished {
            file.arrivals = None;
        }
    }

    /// Whether the bars show: always in a window, and in full screen only while the mouse is
    /// moving.
    fn bars_shown(&self, ctx: &egui::Context) -> bool {
        if !self.full_screen {
            return true;
        }
        let since = self.last_pointer_move.elapsed();
        if since < BARS_LINGER {
            ctx.request_repaint_after(BARS_LINGER - since);
            true
        } else {
            self.seek_bar_held || self.volume_held || self.settings_open
        }
    }

    fn follow_pointer(&mut self, ctx: &egui::Context) {
        let pointer = ctx.input(|input| input.pointer.latest_pos());
        if pointer != self.last_pointer {
            self.last_pointer = pointer;
            self.last_pointer_move = Instant::now();
        }
    }

    /// Gets the picture for this moment, and starts again at the end when looping.
    fn update_picture(&mut self, ctx: &egui::Context) {
        let frame = {
            let Some(file) = &self.file else {
                return;
            };
            let lookup = Self::lookup(&file.info);
            self.preview.update(file.tracks.played_videos(), &lookup)
        };
        if let Some(frame) = frame {
            if self.picture.is_none() {
                report_first_picture(ctx);
            }
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.rgba,
            );
            match &mut self.picture {
                Some(picture) => picture.set(image, TextureOptions::LINEAR),
                None => {
                    self.picture = Some(ctx.load_texture("viewer", image, TextureOptions::LINEAR));
                }
            }
        }
        if let Some(error) = self.preview.take_error() {
            self.notice = Some(error);
        }
        let playing = self.preview.is_playing();
        let ended = self.was_playing && !playing && self.position() >= self.duration();
        self.was_playing = playing;
        if ended && self.looping {
            self.seek(Ticks(0));
            self.play();
            self.was_playing = true;
        }
        if self.preview.is_playing() {
            ctx.request_repaint();
        }
    }

    /// Dragging the seek bar holds playback still, and lets it go on afterwards.
    fn hold_playback_while_scrubbing(&mut self) {
        if self.seek_bar_held && self.is_playing() {
            self.pause();
            self.resume_after_scrub = true;
        } else if !self.seek_bar_held
            && std::mem::take(&mut self.resume_after_scrub)
            && self.position() < self.duration()
        {
            self.play();
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Letters typed into a box in the info panel are not commands.
        let typing = self.info.as_ref().is_some_and(info::InfoPanel::is_typing);
        if typing || ctx.egui_wants_keyboard_input() {
            return;
        }
        let consume = |modifiers, key| ctx.input_mut(|input| input.consume_key(modifiers, key));
        if self.settings_open {
            if consume(Modifiers::NONE, Key::Escape) {
                self.settings_open = false;
            }
            return;
        }
        if self.confirm_delete {
            return;
        }
        if self.file.is_some() {
            if consume(Modifiers::NONE, Key::Delete) {
                self.ask_to_delete();
            }
            if consume(Modifiers::NONE, Key::Space) || consume(Modifiers::NONE, Key::K) {
                self.toggle_playback();
            }
            if consume(Modifiers::NONE, Key::ArrowLeft) {
                self.skip(false);
            }
            if consume(Modifiers::NONE, Key::ArrowRight) {
                self.skip(true);
            }
            if consume(Modifiers::NONE, Key::Comma) {
                self.step_frame(false);
            }
            if consume(Modifiers::NONE, Key::Period) {
                self.step_frame(true);
            }
            if consume(Modifiers::NONE, Key::Home) {
                self.seek(Ticks(0));
            }
            if consume(Modifiers::NONE, Key::End) {
                self.seek(self.duration());
            }
        }
        if consume(Modifiers::NONE, Key::ArrowUp) {
            self.set_volume((self.volume() + VOLUME_STEP).min(100));
            self.save_settings();
        }
        if consume(Modifiers::NONE, Key::ArrowDown) {
            self.set_volume(self.volume().saturating_sub(VOLUME_STEP));
            self.save_settings();
        }
        if consume(Modifiers::NONE, Key::M) {
            self.toggle_mute();
            self.save_settings();
        }
        if consume(Modifiers::NONE, Key::F) && self.mini.is_none() {
            let full = !self.full_screen;
            self.set_full_screen(ctx, full);
        }
        if consume(Modifiers::NONE, Key::PageUp) {
            self.go_to_neighbour(ctx, false);
        }
        if consume(Modifiers::NONE, Key::PageDown) {
            self.go_to_neighbour(ctx, true);
        }
        if consume(Modifiers::NONE, Key::Num0) {
            self.fit();
        }
        if consume(Modifiers::COMMAND, Key::Equals) || consume(Modifiers::COMMAND, Key::Plus) {
            self.zoom_by(ctx, zoom::STEP, None);
        }
        if consume(Modifiers::COMMAND, Key::Minus) {
            self.zoom_by(ctx, 1.0 / zoom::STEP, None);
        }
        if self.trim.is_some() {
            if consume(Modifiers::NONE, Key::OpenBracket) {
                self.set_trim_start(self.position());
            }
            if consume(Modifiers::NONE, Key::CloseBracket) {
                self.set_trim_end(self.position());
            }
        }
        if consume(Modifiers::NONE, Key::Escape) {
            if self.trim.is_some() {
                self.close_trimmer();
            } else if self.mini.is_some() {
                self.leave_mini_view(ctx);
            } else if self.full_screen {
                self.set_full_screen(ctx, false);
            }
        }
    }

    /// The picture, or the cover or mark of a sound file, or why the file can't be played.
    fn video_area(&mut self, ui: &mut egui::Ui, palette: &Palette, area: Rect) {
        ui.painter().rect_filled(area, 0.0, palette.stage);
        let ctx = ui.ctx().clone();
        if let Some(video) = self.video_size()
            && self.view != area
        {
            // A resized window keeps the zoom, held in, or goes back to fit if that is larger.
            self.zoom = self.zoom.for_view(area, video, ctx.pixels_per_point());
        }
        self.view = area;
        let response = ui.interact(
            area,
            egui::Id::unique("viewer_video"),
            Sense::click_and_drag(),
        );
        self.picture_input(&ctx, &response);
        if let Some(failed) = &self.failed {
            let painter = ui.painter_at(area);
            painter.text(
                area.center() - vec2(0.0, 12.0),
                Align2::CENTER_CENTER,
                "This file can't be played",
                fonts::strong(18.0),
                palette_on_stage(palette),
            );
            painter.text(
                area.center() + vec2(0.0, 16.0),
                Align2::CENTER_CENTER,
                &failed.reason,
                FontId::proportional(13.0),
                palette_on_stage(palette).gamma_multiply(0.75),
            );
            return;
        }
        let Some(file) = &self.file else {
            // While the file is let go for a change, its last picture stays.
            if let Some(picture) = &self.picture {
                let rect = Rect::from_center_size(
                    area.center(),
                    fit_inside(picture.size_vec2(), area.size()),
                );
                ui.painter_at(area).image(
                    picture.id(),
                    rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                return;
            }
            ui.painter().text(
                area.center(),
                Align2::CENTER_CENTER,
                "Open a video or sound from File Explorer to play it here.",
                FontId::proportional(14.0),
                palette_on_stage(palette).gamma_multiply(0.75),
            );
            return;
        };
        if file.is_sound() {
            let cover = file.cover.clone();
            let has_cover = cover.is_some();
            let picture = cover.or_else(|| self.mark(ui.ctx()));
            if let Some(picture) = picture {
                let size = picture.size_vec2();
                let room = if has_cover {
                    area.shrink(24.0).size()
                } else {
                    vec2(MARK_SIDE, MARK_SIDE)
                };
                let shown = fit_inside(size, room);
                let rect = Rect::from_center_size(area.center(), shown);
                ui.painter().image(
                    picture.id(),
                    rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            return;
        }
        if let Some(picture) = &self.picture {
            let size = vec2(file.info.video.width as f32, file.info.video.height as f32);
            let rect = self.zoom.picture_rect(area, size);
            ui.painter_at(area).image(
                picture.id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        self.keep_sharp(&ctx);
    }

    /// What the mouse does on the picture: a click plays or pauses unless zoomed, the wheel
    /// zooms toward the pointer, a drag pans while zoomed, and a double-click goes back to fit.
    /// In mini view a drag moves the window instead.
    fn picture_input(&mut self, ctx: &egui::Context, response: &egui::Response) {
        if self.file.is_none() {
            return;
        }
        if self.mini.is_some() {
            if response.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            } else if response.clicked() {
                self.toggle_playback();
            }
            return;
        }
        if response.double_clicked() {
            self.fit();
        } else if response.clicked() && self.zoom.is_fit() {
            self.toggle_playback();
        }
        if response.drag_started() {
            self.panning = !self.zoom.is_fit();
        }
        if self.panning
            && response.dragged()
            && let Some(video) = self.video_size()
        {
            self.zoom = self.zoom.pan(
                self.view,
                video,
                response.drag_delta(),
                ctx.pixels_per_point(),
            );
        }
        if !response.dragged() {
            self.panning = false;
        }
        if response.hovered() {
            let notches = ctx.input(|input| wheel_notches(&input.events));
            if notches != 0.0 {
                let pointer = ctx.input(|input| input.pointer.hover_pos());
                self.zoom_by(ctx, zoom::STEP.powf(notches), pointer);
            }
        }
    }

    /// The BareClip mark, which stands in for a sound file's missing cover.
    fn mark(&mut self, ctx: &egui::Context) -> Option<TextureHandle> {
        if self.mark.is_none() {
            let icon = eframe::icon_data::from_png_bytes(crate::editor::LOGO_DARK).ok()?;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            self.mark = Some(ctx.load_texture("viewer-mark", image, TextureOptions::LINEAR));
        }
        self.mark.clone()
    }

    /// The strip across the top, which replaces the Windows title bar and does its jobs.
    fn title_strip(&mut self, ui: &mut egui::Ui, palette: &Palette, rect: Rect) {
        let ctx = ui.ctx().clone();
        ui.painter().rect_filled(rect, 0.0, palette.bg_app);
        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        let strip = ui.interact(
            rect,
            egui::Id::unique("viewer_title_drag"),
            Sense::click_and_drag(),
        );
        if strip.double_clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        } else if strip.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        let has_file = self.file.is_some();
        let inner = rect.shrink2(vec2(8.0, 0.0));
        let left = ui
            .scope_builder(
                UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                    let trimming = self.trim.is_some();
                    if ui
                        .add_enabled(
                            has_file,
                            ActionButton::new(palette, Kind::Ghost, "Trim")
                                .icon(icons::SCISSORS)
                                .selected(trimming),
                        )
                        .on_hover_text("Keep one stretch of this file")
                        .clicked()
                    {
                        if trimming {
                            self.close_trimmer();
                        } else {
                            let ctx = ui.ctx().clone();
                            self.open_trimmer(&ctx);
                        }
                    }
                    if ui
                        .add_enabled(
                            has_file,
                            IconButton::new(palette, Kind::Ghost, icons::TRASH_2, "Delete"),
                        )
                        .on_hover_text("Move to the Recycle Bin (Delete)")
                        .clicked()
                    {
                        self.ask_to_delete();
                    }
                    if ui
                        .add_enabled(
                            has_file,
                            IconButton::new(
                                palette,
                                Kind::Ghost,
                                icons::CLAPPERBOARD,
                                "Edit in BareClip",
                            ),
                        )
                        .on_hover_text("Edit in BareClip")
                        .clicked()
                    {
                        self.edit_in_bareclip();
                    }
                },
            )
            .response
            .rect;
        let right = ui
            .scope_builder(
                UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::right_to_left(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                    let button = |icon, name| {
                        IconButton::new(palette, Kind::Ghost, icon, name)
                            .size(WINDOW_BUTTON_SIZE, 16.0)
                    };
                    if ui
                        .add(button(icons::X, "Close window"))
                        .on_hover_text("Close")
                        .clicked()
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    let (icon, name, hint) = if maximized {
                        (icons::COPY, "Restore window", "Restore")
                    } else {
                        (icons::SQUARE, "Maximise window", "Maximise")
                    };
                    if ui.add(button(icon, name)).on_hover_text(hint).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                    }
                    if ui
                        .add(button(icons::MINUS, "Minimise window"))
                        .on_hover_text("Minimise")
                        .clicked()
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                    ui.add_space(8.0);
                    if ui
                        .add(IconButton::new(
                            palette,
                            Kind::Ghost,
                            icons::SETTINGS,
                            "Settings",
                        ))
                        .on_hover_text("Settings")
                        .clicked()
                    {
                        self.settings_open = true;
                    }
                    let info_open = self.info.is_some();
                    if ui
                        .add_enabled(
                            has_file || info_open,
                            IconButton::new(palette, Kind::Ghost, icons::INFO, "Info").tint(
                                if info_open {
                                    palette.accent
                                } else {
                                    palette.text
                                },
                            ),
                        )
                        .on_hover_text("Info")
                        .clicked()
                    {
                        self.toggle_info();
                    }
                },
            )
            .response
            .rect;
        // The name sits in the middle of the window, as far as the buttons allow.
        let room = (right.left() - left.right() - 24.0).max(0.0);
        let name_rect = Rect::from_center_size(
            pos2(rect.center().x, rect.center().y),
            vec2(room.min(rect.width()), TITLE_HEIGHT),
        );
        let name_rect = name_rect
            .translate(vec2((left.right() + 12.0 - name_rect.left()).max(0.0), 0.0))
            .translate(vec2(
                (right.left() - 12.0 - name_rect.right()).min(0.0),
                0.0,
            ));
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(name_rect)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
            |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(self.name())
                            .font(FontId::proportional(components::NAME_BAR_TEXT - 1.0))
                            .color(palette.text),
                    )
                    .truncate()
                    .selectable(false),
                );
            },
        );
    }

    /// The seek line, and under it the file's details, the controls and zoom.
    fn bottom_bar(&mut self, ui: &mut egui::Ui, palette: &Palette, rect: Rect) {
        ui.painter().rect_filled(rect, 0.0, palette.bg_app);
        ui.painter()
            .hline(rect.x_range(), rect.top(), Stroke::new(1.0, palette.border));
        let controls = Rect::from_min_max(
            pos2(rect.left(), rect.bottom() - CONTROL_ROW_HEIGHT),
            rect.max,
        );
        let has_file = self.file.is_some();
        let duration = self.duration();
        let position = self.position();

        if self.trim.is_some() {
            // The trimmer's strip and buttons take the seek line's place.
            let strip = Rect::from_min_size(rect.min, vec2(rect.width(), trim::TRIM_ROW_HEIGHT));
            let actions = Rect::from_min_size(
                pos2(rect.left(), strip.bottom()),
                vec2(rect.width(), trim::ACTION_ROW_HEIGHT),
            );
            self.trim_row(ui, palette, strip);
            self.trim_actions(ui, palette, actions);
        } else {
            let seek_row = Rect::from_min_size(rect.min, vec2(rect.width(), SEEK_ROW_HEIGHT));
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(seek_row.shrink2(vec2(12.0, 0.0)))
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    time_label(ui, palette, &format_timecode(position), Align::Max);
                    let room = (ui.available_width() - TIME_WIDTH - 8.0).max(40.0);
                    if let Some(time) = crate::timeline::seek_bar(
                        ui,
                        palette,
                        room,
                        position,
                        duration,
                        &mut self.seek_bar_held,
                    ) {
                        self.seek(time);
                    }
                    time_label(ui, palette, &format_timecode(duration), Align::Min);
                },
            );
        }

        // The file's details on the left.
        let details = self.file.as_ref().map(file_details).unwrap_or_default();
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(controls.shrink2(vec2(14.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                for detail in details {
                    ui.add(
                        egui::Label::new(
                            RichText::new(detail).size(12.0).color(palette.text_muted),
                        )
                        .selectable(false),
                    );
                }
            },
        );

        // The controls in the middle.
        let middle =
            Rect::from_center_size(controls.center(), vec2(MIDDLE_WIDTH, CONTROL_ROW_HEIGHT));
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(middle)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                let (icon, name) = if self.is_playing() {
                    (icons::PAUSE, "Pause")
                } else {
                    (icons::PLAY, "Play")
                };
                let play = IconButton::new(palette, Kind::Primary, icon, name)
                    .size(36.0, 16.0)
                    .round()
                    .solid();
                if ui
                    .add_enabled(has_file, play)
                    .on_hover_text("Play or pause (Space)")
                    .clicked()
                {
                    self.toggle_playback();
                }
                ui.add_space(6.0);
                let (icon, name) = if self.is_muted() || self.volume() == 0 {
                    (icons::VOLUME_X, "Unmute")
                } else {
                    (icons::VOLUME_2, "Mute")
                };
                if ui
                    .add(IconButton::new(palette, Kind::Ghost, icon, name))
                    .on_hover_text("Mute (M)")
                    .clicked()
                {
                    self.toggle_mute();
                    self.save_settings();
                }
                let shown = if self.is_muted() { 0 } else { self.volume() };
                let mut held = self.volume_held;
                if let Some(volume) =
                    level_bar(ui, palette, VOLUME_BAR_WIDTH, shown, &mut held, "Volume")
                {
                    self.set_volume(volume);
                }
                if self.volume_held && !held {
                    self.save_settings();
                }
                self.volume_held = held;
                ui.add_space(6.0);
                self.speed_button(ui, palette);
                let looping = self.looping;
                let loop_button = IconButton::new(palette, Kind::Ghost, icons::REPEAT, "Loop")
                    .tint(if looping {
                        palette.accent
                    } else {
                        palette.text
                    });
                if ui
                    .add_enabled(has_file, loop_button)
                    .on_hover_text(if looping { "Loop is on" } else { "Loop" })
                    .clicked()
                {
                    self.looping = !looping;
                }
                let picture = self.video_size().is_some();
                let save = IconButton::new(palette, Kind::Ghost, icons::CAMERA, "Save picture")
                    .hidden(!picture);
                if ui
                    .add_enabled(picture && self.saving_picture.is_none(), save)
                    .on_hover_text("Save this picture beside the video")
                    .clicked()
                {
                    self.save_picture(ui.ctx());
                }
                if ui
                    .add_enabled(
                        has_file,
                        IconButton::new(
                            palette,
                            Kind::Ghost,
                            icons::PICTURE_IN_PICTURE_2,
                            "Mini view",
                        ),
                    )
                    .on_hover_text("Mini view, a small window above the others")
                    .clicked()
                {
                    self.enter_mini_view(ui.ctx());
                }
                let (icon, name) = if self.full_screen {
                    (icons::MINIMIZE, "Leave full screen")
                } else {
                    (icons::MAXIMIZE, "Full screen")
                };
                if ui
                    .add(IconButton::new(palette, Kind::Ghost, icon, name))
                    .on_hover_text(format!("{name} (F)"))
                    .clicked()
                {
                    let full = !self.full_screen;
                    self.set_full_screen(ui.ctx(), full);
                }
            },
        );

        if self.video_size().is_some() {
            self.zoom_controls(ui, palette, controls);
        }
    }

    /// Zoom out, the zoom's slider, zoom in, the percentage and fit, at the bar's right.
    fn zoom_controls(&mut self, ui: &mut egui::Ui, palette: &Palette, controls: Rect) {
        let ctx = ui.ctx().clone();
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(controls.shrink2(vec2(10.0, 0.0)))
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                if ui
                    .add(ActionButton::new(palette, Kind::Ghost, "Fit").small())
                    .on_hover_text("Fit the whole picture (0)")
                    .clicked()
                {
                    self.fit();
                }
                let percent = self.zoom_percent(&ctx);
                ui.allocate_ui_with_layout(
                    vec2(44.0, CONTROL_ROW_HEIGHT),
                    Layout::right_to_left(Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{percent:.0}%"))
                                    .size(12.0)
                                    .color(palette.text_muted),
                            )
                            .selectable(false),
                        );
                    },
                );
                if ui
                    .add(IconButton::new(
                        palette,
                        Kind::Ghost,
                        icons::ZOOM_IN,
                        "Zoom in",
                    ))
                    .on_hover_text("Zoom in (Ctrl+=)")
                    .clicked()
                {
                    self.zoom_by(&ctx, zoom::STEP, None);
                }
                // The slider runs from fit to the closest zoom, evenly in steps of zoom.
                let fit = self.video_size().map_or(100.0, |video| {
                    zoom::fit_percent(self.view, video, ctx.pixels_per_point())
                });
                let most = (zoom::MOST_ZOOM * 100.0).max(fit);
                let share = if most > fit {
                    ((percent / fit).ln() / (most / fit).ln()).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let mut held = false;
                let level = (share * 100.0).round() as u32;
                if let Some(level) = level_bar(ui, palette, 100.0, level, &mut held, "Zoom")
                    && let Some(video) = self.video_size()
                {
                    let wanted = fit * (most / fit).powf(level as f32 / 100.0);
                    self.zoom =
                        self.zoom
                            .zoom_to_percent(self.view, video, wanted, ctx.pixels_per_point());
                }
                if ui
                    .add(IconButton::new(
                        palette,
                        Kind::Ghost,
                        icons::ZOOM_OUT,
                        "Zoom out",
                    ))
                    .on_hover_text("Zoom out (Ctrl+-)")
                    .clicked()
                {
                    self.zoom_by(&ctx, 1.0 / zoom::STEP, None);
                }
            },
        );
    }

    /// Saves the picture on screen beside the video, at the video's full size, in the
    /// background.
    pub fn save_picture(&mut self, ctx: &egui::Context) {
        let Some(file) = self.file.as_ref().filter(|file| !file.is_sound()) else {
            return;
        };
        if self.saving_picture.is_some() {
            return;
        }
        let video = file.info.path.clone();
        let time = self.position();
        let target = text::picture_file_name(&video, time);
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        thread::spawn(move || {
            let saved = bareclip_media::picture_at(&video, time)
                .and_then(|picture| bareclip_media::write_png(&target, &picture))
                .map(|()| target)
                .map_err(|error| format!("The picture couldn't be saved. {error}"));
            let _ = sender.send(saved);
            repaint.request_repaint();
        });
        self.saving_picture = Some(receiver);
    }

    fn take_saved_picture(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.saving_picture else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(path)) => {
                self.saved_picture = Some((path, Instant::now()));
                self.saving_picture = None;
                ctx.request_repaint_after(SAVED_NOTE_FOR);
            }
            Ok(Err(problem)) => {
                self.notice = Some(problem);
                self.saving_picture = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.saving_picture = None,
        }
    }

    /// Says a picture was saved, with Show in folder, for a few seconds.
    fn saved_picture_note(&mut self, ui: &mut egui::Ui, palette: &Palette, area: Rect) {
        let Some((path, since)) = &self.saved_picture else {
            return;
        };
        if since.elapsed() >= SAVED_NOTE_FOR {
            self.saved_picture = None;
            return;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let path = path.clone();
        let rect = Rect::from_center_size(
            pos2(area.center().x, area.top() + 36.0),
            vec2(area.width().min(520.0), 40.0),
        );
        ui.painter().rect_filled(rect, 8.0, palette.raised);
        let mut close = false;
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(12.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                components::symbol(ui, &icons::CIRCLE_CHECK, 16.0, palette.success);
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("Saved {name}"))
                            .size(13.0)
                            .color(palette.text),
                    )
                    .truncate()
                    .selectable(false),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(IconButton::new(
                            palette,
                            Kind::Ghost,
                            icons::X,
                            "Close note",
                        ))
                        .clicked()
                    {
                        close = true;
                    }
                    if ui
                        .add(ActionButton::new(palette, Kind::Secondary, "Show in folder").small())
                        .clicked()
                    {
                        show_in_folder(&path);
                    }
                });
            },
        );
        if close {
            self.saved_picture = None;
        }
    }

    /// The speed's button, which shows the speed when it isn't 1x, and its menu.
    fn speed_button(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let label = if self.speed == NORMAL_SPEED {
            "1x".to_owned()
        } else {
            text::speed_name(self.speed())
        };
        let button = ActionButton::new(palette, Kind::Ghost, &label)
            .icon(icons::GAUGE)
            .small()
            .selected(self.speed != NORMAL_SPEED);
        let response = ui
            .add_enabled(self.file.is_some(), button)
            .on_hover_text("Watching speed");
        let mut chosen = None;
        egui::Popup::menu(&response).show(|ui| {
            for (index, speed) in SPEEDS.iter().enumerate().rev() {
                let name = text::speed_name(*speed);
                if ui
                    .add(MenuItem::choice(palette, &name, index == self.speed))
                    .clicked()
                {
                    chosen = Some(index);
                }
            }
        });
        if let Some(index) = chosen {
            self.set_speed(index);
        }
    }

    /// The player's small Settings window.
    fn settings_window(&mut self, ctx: &egui::Context, palette: &Palette) {
        if !self.settings_open {
            return;
        }
        let mut close = false;
        let mut step = self.seek_step();
        let mut resume = self.settings.resume;
        let mut theme = self.theme;
        egui::Modal::new(egui::Id::unique("viewer_settings"))
            .frame(crate::editor::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(440.0);
                components::dialog_title(ui, &icons::SETTINGS, palette.accent, "Player settings");
                ui.add_space(12.0);
                components::settings_group(ui, palette, "Playing", |ui| {
                    components::settings_row(
                        ui,
                        palette,
                        (
                            "Arrow keys jump",
                            Some("How far Left and Right jump, from 1 to 60 seconds"),
                        ),
                        90.0,
                        |ui| {
                            ui.add(
                                egui::DragValue::new(&mut step)
                                    .range(storage::SEEK_SECONDS_RANGE)
                                    .suffix(" s")
                                    .speed(0.2),
                            )
                            .on_hover_text("How far Left and Right jump, from 1 to 60 seconds");
                        },
                    );
                    components::settings_divider(ui, palette);
                    let row =
                        components::SwitchRow::new(palette, "Pick up where I stopped", resume);
                    if ui.add(row).clicked() {
                        resume = !resume;
                    }
                });
                ui.add_space(8.0);
                components::settings_group(ui, palette, "Look", |ui| {
                    components::settings_row(ui, palette, ("Theme", None), 230.0, |ui| {
                        for (choice, name) in [
                            (ThemeSetting::System, "System"),
                            (ThemeSetting::Light, "Light"),
                            (ThemeSetting::Dark, "Dark"),
                        ] {
                            if ui
                                .add(
                                    ActionButton::new(palette, Kind::Secondary, name)
                                        .small()
                                        .selected(theme == choice),
                                )
                                .clicked()
                            {
                                theme = choice;
                            }
                        }
                    });
                });
                ui.add_space(8.0);
                components::settings_group(ui, palette, "Keys", |ui| {
                    for (keys, what) in KEY_LIST {
                        components::compact_settings_row(ui, palette, what, 130.0, |ui| {
                            ui.colored_label(palette.text_muted, *keys);
                        });
                    }
                });
                ui.add_space(14.0);
                components::dialog_buttons(ui, |ui| {
                    close = ui
                        .add(ActionButton::new(palette, Kind::Primary, "Done"))
                        .clicked();
                });
            });
        if step != self.seek_step() {
            self.set_seek_step(step);
        }
        if resume != self.settings.resume {
            self.set_resume(resume);
        }
        if theme != self.theme {
            self.theme = theme;
            ctx.set_theme(egui::ThemePreference::from(theme));
            self.save_settings();
        }
        if close {
            self.settings_open = false;
        }
    }

    /// A problem, shown over the bottom of the picture.
    fn show_notice(&mut self, ui: &mut egui::Ui, palette: &Palette, area: Rect) {
        let Some(notice) = &self.notice else {
            return;
        };
        let galley = ui.painter().layout(
            notice.clone(),
            FontId::proportional(13.0),
            palette.text,
            (area.width() - 80.0).max(100.0),
        );
        let rect = Rect::from_center_size(
            pos2(
                area.center().x,
                area.bottom() - 32.0 - galley.size().y / 2.0,
            ),
            galley.size() + vec2(24.0, 14.0),
        );
        ui.painter().rect_filled(rect, 8.0, palette.raised);
        ui.painter()
            .galley(rect.min + vec2(12.0, 7.0), galley, palette.text);
    }
}

/// How many notches the wheel turned this frame, up counting as positive, from the raw wheel
/// events, which egui's smoothed scrolling would spread over several frames.
fn wheel_notches(events: &[egui::Event]) -> f32 {
    events
        .iter()
        .map(|event| match event {
            egui::Event::MouseWheel { unit, delta, .. } => match unit {
                egui::MouseWheelUnit::Point => delta.y / WHEEL_NOTCH,
                egui::MouseWheelUnit::Line => delta.y,
                egui::MouseWheelUnit::Page => delta.y * 3.0,
            },
            _ => 0.0,
        })
        .sum()
}

/// Opens File Explorer with a file selected.
fn show_in_folder(path: &Path) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn();
}

/// Names a file to write how long the first picture took into, for timing a real launch. The
/// window closes once it is written.
pub const FIRST_PICTURE_VARIABLE: &str = "BARECLIP_FIRST_PICTURE_FILE";

/// For a timed launch, writes the milliseconds from when Windows started the process to now,
/// and closes the window.
fn report_first_picture(ctx: &egui::Context) {
    let Some(report) = std::env::var_os(FIRST_PICTURE_VARIABLE) else {
        return;
    };
    if let Some(since) = since_process_start() {
        let _ = std::fs::write(report, format!("{}", since.as_millis()));
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}

/// How long ago Windows created this process.
fn since_process_start() -> Option<Duration> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::{
            SystemInformation::GetSystemTimeAsFileTime,
            Threading::{GetCurrentProcess, GetProcessTimes},
        },
    };
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user, mut now) = (zero, zero, zero, zero, zero);
    // SAFETY: every pointer is to a local that outlives the calls.
    let read = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    if read == 0 {
        return None;
    }
    unsafe { GetSystemTimeAsFileTime(&mut now) };
    let ticks =
        |time: FILETIME| u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime);
    // File times count tenths of a microsecond.
    Some(Duration::from_nanos(
        ticks(now).saturating_sub(ticks(created)) * 100,
    ))
}

/// The keys, as Settings lists them.
const KEY_LIST: &[(&str, &str)] = &[
    ("Space, K", "Play or pause"),
    ("Left, Right", "Jump back or forward"),
    (", and .", "One frame back or forward"),
    ("Up, Down", "Volume"),
    ("M", "Mute"),
    ("Home, End", "Start or end"),
    ("Page Up, Page Down", "Previous or next file"),
    ("0, Ctrl+=, Ctrl+-", "Fit, zoom in or out"),
    ("F, Escape", "Full screen"),
];

/// One of the watching speeds, keeping the voice's pitch.
fn watching_speed(index: usize) -> Speed {
    Speed {
        keep_pitch: true,
        ..Speed::default()
    }
    .with_factor(SPEEDS[index.min(SPEEDS.len() - 1)])
}

/// Text on the stage, which is dark in both themes.
fn palette_on_stage(palette: &Palette) -> Color32 {
    let _ = palette;
    theme::DARK.text
}

/// A size shrunk or grown to fit inside a room, keeping its shape.
pub fn fit_inside(size: egui::Vec2, room: egui::Vec2) -> egui::Vec2 {
    if size.x <= 0.0 || size.y <= 0.0 {
        return vec2(0.0, 0.0);
    }
    let scale = (room.x / size.x).min(room.y / size.y);
    size * scale
}

/// The format, size on screen and size on disk, as the bottom bar's left shows them.
fn file_details(file: &OpenFile) -> Vec<String> {
    let format = file
        .info
        .path
        .extension()
        .map(|extension| extension.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| file.info.container.to_uppercase());
    let mut details = vec![format];
    if !file.is_sound() {
        details.push(format!(
            "{} x {}",
            file.info.video.width, file.info.video.height
        ));
    }
    details.push(text::file_size(file.info.size_bytes));
    details
}

fn time_label(ui: &mut egui::Ui, palette: &Palette, time: &str, align: Align) {
    ui.allocate_ui_with_layout(
        vec2(TIME_WIDTH, SEEK_ROW_HEIGHT),
        Layout::left_to_right(Align::Center).with_main_align(align),
        |ui| {
            ui.add(
                egui::Label::new(RichText::new(time).size(12.0).color(palette.text_muted))
                    .selectable(false),
            );
        },
    );
}

/// A bar for a level out of 100, such as the volume, which a click or drag sets.
fn level_bar(
    ui: &mut egui::Ui,
    palette: &Palette,
    width: f32,
    level: u32,
    held: &mut bool,
    name: &str,
) -> Option<u32> {
    const KNOB: f32 = 6.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, name));
    let left = rect.left() + KNOB;
    let room = (rect.width() - KNOB * 2.0).max(1.0);
    let line = Rect::from_center_size(rect.center(), vec2(room, 3.0));
    ui.painter().rect_filled(line, 1.5, palette.border_strong);
    let x = left + room * level.min(100) as f32 / 100.0;
    ui.painter().rect_filled(
        Rect::from_min_max(line.min, pos2(x, line.bottom())),
        1.5,
        palette.accent,
    );
    ui.painter()
        .circle_filled(pos2(x, rect.center().y), KNOB, palette.accent);
    *held = response.dragged();
    if !(response.dragged() || response.clicked()) {
        return None;
    }
    let pointer = response.interact_pointer_pos()?;
    let share = ((pointer.x - left) / room).clamp(0.0, 1.0);
    Some((share * 100.0).round() as u32)
}

mod browse;
mod info;
mod mini;
mod resume;
#[cfg(test)]
mod tests;
mod trim;
mod zoom;
