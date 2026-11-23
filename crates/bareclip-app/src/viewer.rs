//! The player: what a double-click on a video or sound opens. It is a window of its own, laid
//! out as the Photos app's: a title strip, the picture, a status bar along the bottom, and a
//! pill of playing controls floating over the bottom of the picture.
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
    time::{Ticks, clock_needs_hours, format_clock},
    timeline::{Clip, ClipId, MediaId, TimeRange},
    tracks::Tracks,
};
use bareclip_media::{MediaInfo, probe};
use eframe::egui::{
    self, Align2, Color32, FontId, Key, Modifiers, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, pos2, vec2,
};

use note::{Note, NotePress};
use windows_sys::Win32::Foundation::HWND;
use zoom::Zoom;

use crate::{
    components::{self, ActionButton, Kind, MenuItem},
    fonts, icons,
    preview::Preview,
    storage::{self, Settings, ThemeSetting, ViewerSettings, WindowPlace},
    text, theme,
    theme::Palette,
    window_place,
};

/// The smallest the player's window goes, which leaves room for mini view.
pub const SMALLEST_WINDOW: [f32; 2] = [320.0, 180.0];
/// The watching speeds, slowest first.
pub const SPEEDS: [f64; 7] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
/// Where 1x is in `SPEEDS`.
const NORMAL_SPEED: usize = 3;
/// How long the pill stays once the mouse stops moving while playing, and while paused.
const PILL_LINGER: Duration = Duration::from_secs(2);
const PILL_LINGER_PAUSED: Duration = Duration::from_secs(5);
/// Keys that play, pause or move through the video, which close an open flyout.
const PLAYBACK_KEYS: [Key; 10] = [
    Key::Space,
    Key::K,
    Key::ArrowLeft,
    Key::ArrowRight,
    Key::Comma,
    Key::Period,
    Key::Home,
    Key::End,
    Key::PageUp,
    Key::PageDown,
];
/// How much Up and Down change the volume, out of 100.
const VOLUME_STEP: u32 = 5;
/// A step forward or back when the frame table isn't read yet, or there's no picture.
const FALLBACK_STEP_SECONDS: f64 = 1.0 / 30.0;

/// As tall as the taller of Settings' pages, so it keeps its size when tabs are switched.
const SETTINGS_PAGE_HEIGHT: f32 = 380.0;
/// The room between Settings' title, groups and page.
const SETTINGS_GAP: f32 = 14.0;
/// The room Settings leaves for its title, its Done button and the window's edges, so its
/// page scrolls rather than running off a short window.
const SETTINGS_FRAME_ROOM: f32 = 170.0;
/// How tall a line of Settings' key list is.
const KEY_ROW_HEIGHT: f32 = 34.0;
/// Room for the Make default button, which the row's words stop short of.
const MAKE_DEFAULT_WIDTH: f32 = 104.0;
/// As tall as the small Make default button it stands in for, so the row keeps its height.
const DEFAULT_MARK_HEIGHT: f32 = 26.0;
/// Under Default player, once every type the player plays opens in BareClip.
const DEFAULT_PLAYER_NOTE: &str = "BareClip is your default player";
/// Under Default player, while any type opens in another program.
const NOT_DEFAULT_PLAYER_NOTE: &str = "Some files open in another app";
/// The jumps' dropdowns, wide enough for "30 s" and the arrow.
const JUMP_DROPDOWN_WIDTH: f32 = 84.0;
/// The symbol over the message shown when nothing can play.
const STAGE_SYMBOL: f32 = 32.0;
/// The brand's logo with its name beside it, which stands in for a sound file's missing cover:
/// gold on the dark theme and bronze on the light one, as the brand has them.
const LOCKUP_DARK: &[u8] =
    include_bytes!("../../../assets/brand/png/lockup-horizontal-dark-1600.png");
const LOCKUP_LIGHT: &[u8] =
    include_bytes!("../../../assets/brand/png/lockup-horizontal-light-1600.png");
/// How wide the logo and name show at most.
const LOCKUP_WIDTH: f32 = 520.0;
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
    /// The logo and name for a sound file with no cover, and whether it is the dark theme's.
    lockup: Option<(bool, TextureHandle)>,
    settings: ViewerSettings,
    theme: ThemeSetting,
    app_folder: Option<PathBuf>,
    /// Which of `SPEEDS` is playing.
    speed: usize,
    looping: bool,
    full_screen: bool,
    /// Whether the window was maximised, and where it was, before full screen, so leaving full
    /// screen puts it back and closing in full screen saves that rather than the whole screen.
    before_full_screen: Option<(bool, Option<WindowPlace>)>,
    /// The bottom bar's flyout that is open, if any.
    flyout: bars::Flyouts,
    last_pointer_move: Instant,
    last_pointer: Option<egui::Pos2>,
    seek_bar_held: bool,
    /// Where the pill was last drawn, so resting the mouse on it keeps it shown.
    pill_at: Rect,
    resume_after_scrub: bool,
    volume_held: bool,
    settings_open: bool,
    settings_tab: SettingsTab,
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
    /// The info panel, while it is open. It stays open from file to file.
    info: Option<info::InfoPanel>,
    /// Delete is waiting for an answer.
    confirm_delete: bool,
    /// Moves a file to the Recycle Bin. Tests swap it for a folder of their own.
    trash_file: fn(&Path) -> Result<(), String>,
    /// Whether every type the player plays opens in BareClip. Tests swap it, so they don't
    /// depend on what this PC opens files with.
    is_default_player: fn() -> bool,
    /// Asks where to save a trim, given the suggested file. Tests swap it, so no window opens.
    pick_trim_target: fn(&Path) -> Option<PathBuf>,
    /// What it said while Settings is open, asked again when the window comes back to the
    /// front, as it does after Windows' Default apps page.
    default_player: Option<bool>,
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
            lockup: None,
            settings: ViewerSettings::default(),
            theme: ThemeSetting::System,
            app_folder: None,
            speed: NORMAL_SPEED,
            looping: false,
            full_screen: false,
            before_full_screen: None,
            flyout: bars::Flyouts::default(),
            last_pointer_move: Instant::now(),
            last_pointer: None,
            seek_bar_held: false,
            pill_at: Rect::NOTHING,
            resume_after_scrub: false,
            volume_held: false,
            settings_open: false,
            settings_tab: SettingsTab::General,
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
            info: None,
            confirm_delete: false,
            trash_file: crate::file_ops::move_to_recycle_bin,
            is_default_player: crate::file_ops::is_default_player,
            pick_trim_target: trim::ask_where_to_save,
            default_player: None,
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
        self.app_folder = Some(folder);
    }

    /// Where the player was when it last closed.
    pub fn saved_window(&self) -> Option<WindowPlace> {
        self.settings.window
    }

    /// Keeps where the window is as it closes, for the next run.
    pub fn closing(&mut self, place: Option<WindowPlace>) {
        let place = match self.before_full_screen {
            Some((_, before)) if self.full_screen => before,
            _ => place,
        };
        if let Some(place) = place {
            self.settings.window = Some(place);
        }
        self.save_settings();
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
                // The player carries one clock from file to file, so a new file starts at its
                // beginning only if told to.
                self.seek(Ticks(0));
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
        self.was_playing = false;
        Some(released)
    }

    /// Opens a file again after it was let go, at the same moment, playing or paused as it was,
    /// keeping the zoom and an open trimmer with its handles. It may have a new name.
    pub(super) fn reopen(&mut self, ctx: &egui::Context, path: &Path, at: Ticks, playing: bool) {
        let zoom = self.zoom;
        let trim = self.trim.take();
        self.open(ctx, path);
        self.zoom = zoom;
        if self.file.is_some() {
            self.trim = trim;
        }
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

    /// Goes to the start, carrying on as it was, or to the last frame, where it pauses so the
    /// end can be seen rather than looping or finishing.
    pub fn go_to_end(&mut self, to_end: bool) {
        if to_end {
            self.pause();
            self.seek(self.duration());
        } else {
            self.seek(Ticks(0));
        }
    }

    /// Jumps back or forward as far as Settings says.
    pub fn skip(&mut self, forward: bool) {
        let step = Ticks::from_seconds(f64::from(self.jump(forward)));
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

    /// A time as the player writes it: minutes and seconds, with hours in front for a file an
    /// hour or longer.
    pub fn clock(&self, time: Ticks) -> String {
        format_clock(time, clock_needs_hours(self.duration()))
    }

    /// The watching speed, as a factor.
    #[cfg(test)]
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

    /// How far a jump back or forward goes, in seconds.
    pub fn jump(&self, forward: bool) -> u32 {
        self.settings.jump(forward)
    }

    /// Sets how far a jump back or forward goes, to the nearest choice Settings offers.
    pub fn set_jump(&mut self, forward: bool, seconds: u32) {
        let seconds = storage::nearest_jump(seconds);
        if forward {
            self.settings.jump_forward = seconds;
        } else {
            self.settings.jump_back = seconds;
        }
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

    /// Goes into or out of full screen. A maximised window without Windows' title bar keeps out of
    /// the taskbar's way even in full screen, leaving a strip uncovered, so it stops being
    /// maximised first, and is maximised again on the way out.
    pub(super) fn set_full_screen(&mut self, ctx: &egui::Context, full: bool) {
        if self.full_screen != full {
            self.full_screen = full;
            if full {
                let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                self.before_full_screen =
                    Some((maximized, self.window.and_then(window_place::read)));
                if maximized {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                // Without Windows' title bar the window has a shadow, which Windows makes by moving
                // the picture down a pixel; sending this again after full screen turns that off
                // there, so nothing shows above the top bar, and back on after.
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
                if self
                    .before_full_screen
                    .take()
                    .is_some_and(|(maximized, _)| maximized)
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                }
            }
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
        self.poll_trimmer(&ctx);
        self.keep_within_trim();

        let whole = ui.max_rect();
        if self.mini.is_some() {
            // Mini view is only the picture, with its controls on hover.
            self.video_area(ui, palette, whole);
            self.mini_controls(ui, palette, whole);
            let mut top = whole.top() + note::NOTE_GAP;
            self.show_notice(ui, palette, whole, &mut top);
            return;
        }
        self.flyout.tick(&ctx);
        self.finish_rewrite(&ctx);
        let trimming = self.trim.is_some();
        // In full screen the bars only come over the picture while the mouse is over their
        // strip. In a window, and while trimming, they stay, with the picture between them.
        let overlay = self.full_screen && !trimming;
        let pointer = ctx.input(|input| input.pointer.hover_pos());
        let over = |strip: Rect| pointer.is_some_and(|at| strip.contains(at));
        let title = Rect::from_min_size(whole.min, vec2(whole.width(), bars::TITLE_HEIGHT));
        let bottom_height = if trimming {
            trim::TRIM_BAR_HEIGHT
        } else {
            bars::BOTTOM_HEIGHT
        };
        let bottom = Rect::from_min_max(
            pos2(whole.left(), whole.bottom() - bottom_height),
            whole.max,
        );
        let (top_shown, bottom_shown) = if overlay {
            // A flyout of the bottom bar's keeps it while the mouse goes up onto the flyout.
            let bar_flyout = matches!(
                self.flyout.open,
                Some(bars::Flyout::Zoom | bars::Flyout::Speed)
            );
            let on_pill = pointer.is_some_and(|at| self.pill_at.contains(at));
            (
                theme::appear(&ctx, egui::Id::unique("viewer_top_bar"), over(title)),
                theme::appear(
                    &ctx,
                    egui::Id::unique("viewer_bottom_bar"),
                    (over(bottom) && !on_pill) || bar_flyout,
                ),
            )
        } else {
            (1.0, 1.0)
        };
        let mut video = if overlay {
            whole
        } else {
            Rect::from_min_max(pos2(whole.left(), title.bottom()), bottom.right_top())
        };

        // The info card slides in from the right, between the bars, and the picture narrows
        // beside it as it comes. In full screen it stays clear of where the bars slide in, so
        // they never cover its heading. In a window too narrow to share, it lies over the
        // picture instead.
        let info_shown = theme::appear(
            &ctx,
            egui::Id::unique("viewer_info_card"),
            self.is_info_open(),
        );
        if info_shown <= 0.0 {
            self.forget_closed_info();
        }
        let card = (info_shown > 0.0 && self.info.is_some()).then(|| {
            let (top, mut bottom) = if overlay {
                (title.bottom(), bottom.top())
            } else {
                (video.top(), video.bottom())
            };
            let left = whole.right() - info_shown * (info::CARD_WIDTH + info::CARD_MARGIN);
            if info_narrows_picture(whole.width()) {
                video.max.x =
                    whole.right() - info_shown * (info::CARD_WIDTH + info::CARD_MARGIN * 2.0);
            } else {
                // Lying over the picture, it stops above the pill, or the trimmer's time.
                bottom -= if trimming {
                    trim::FLOATING_ROOM
                } else {
                    pill::PILL_GAP + pill::PILL_HEIGHT
                };
            }
            Rect::from_min_max(
                pos2(left, top + info::CARD_MARGIN),
                pos2(left + info::CARD_WIDTH, bottom - info::CARD_MARGIN),
            )
        });

        // The pill rises above the bottom bar while that shows over the picture.
        let pill_wanted = !trimming && self.file.is_some() && self.pill_shown(&ctx);
        let pill_appears = theme::appear(&ctx, egui::Id::unique("viewer_pill"), pill_wanted);
        let pill_room = video.with_max_y(if overlay {
            video.bottom() - bottom.height() * bottom_shown
        } else {
            video.bottom()
        });
        let pill_rect = pill::pill_rect(pill_room);
        // The trimmer has its own pill, so the player's isn't drawn at all, even fading.
        let pill_appears = if trimming { 0.0 } else { pill_appears };
        self.pill_at = if pill_appears > 0.0 {
            pill_rect
        } else {
            Rect::NOTHING
        };
        // The arrows to the files either side come and go with the pill. Trimming keeps to
        // the file on screen, so they don't show.
        let arrows = if trimming { 0.0 } else { pill_appears };

        self.video_area(ui, palette, video);
        if let Some(card) = card {
            ui.painter().rect_filled(
                Rect::from_min_max(video.right_top(), pos2(whole.right(), video.bottom())),
                0.0,
                palette.bg_app,
            );
            self.info_panel(ui, palette, card);
        }
        if arrows > 0.0 {
            // A card lying over the picture keeps the arrows to its left.
            let arrows_area = match card {
                Some(card) if card.left() < video.right() => {
                    video.with_max_x(card.left() - info::CARD_MARGIN)
                }
                _ => video,
            };
            ui.scope(|ui| {
                ui.multiply_opacity(arrows);
                self.edge_arrows(ui, palette, arrows_area);
            });
        }
        // With the pill away, the pointer goes too while playing, and always in full screen,
        // as in other players.
        if !pill_wanted
            && !trimming
            && (self.is_playing() || self.full_screen)
            && pointer.is_some_and(|at| video.contains(at))
        {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
        if pill_appears > 0.0 {
            let sink = (1.0 - pill_appears) * pill::PILL_SINK;
            ui.scope(|ui| {
                ui.multiply_opacity(pill_appears);
                self.play_pill(ui, palette, pill_rect.translate(vec2(0.0, sink)));
            });
        }
        if top_shown > 0.0 {
            let title_now = title.translate(vec2(0.0, -title.height() * (1.0 - top_shown)));
            self.title_strip(ui, palette, title_now, overlay);
        }
        if trimming {
            bars::bar_backdrop(ui, palette, bottom, false, "viewer_bottom_bar");
            self.trim_bar(ui, palette, bottom, video.x_range());
        } else if bottom_shown > 0.0 {
            let bottom_now = bottom.translate(vec2(0.0, bottom.height() * (1.0 - bottom_shown)));
            self.bottom_bar(ui, palette, bottom_now, overlay);
        }
        // Notes stack down from the top of the picture.
        let mut top = video.top() + note::NOTE_GAP;
        self.show_notice(ui, palette, video, &mut top);
        self.trim_notes(ui, palette, video, &mut top);
        self.saved_picture_note(ui, palette, video, &mut top);
        self.settings_window(&ctx, palette);
        self.delete_dialog(&ctx, palette);
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

    /// Zooms to the next multiple of 10% up or down, as − and + do, between a quarter of fit
    /// and the closest zoom, stopping at fit on the way through.
    pub fn zoom_step(&mut self, ctx: &egui::Context, up: bool) {
        let Some(video) = self.video_size() else {
            return;
        };
        let step = bars::ZOOM_BUTTON_STEP;
        let now = self.zoom_percent(ctx);
        // A hair's allowance, so 90% that is really 89.999% still goes on to 100%.
        let steps = now / step;
        let wanted = if up {
            ((steps + 0.01).floor() + 1.0) * step
        } else {
            ((steps - 0.01).ceil() - 1.0) * step
        };
        let fit = zoom::fit_percent(self.view, video, ctx.pixels_per_point());
        let passes_fit = (now - fit) * (wanted - fit) < 0.0 && (now - fit).abs() > 0.5;
        if passes_fit {
            self.fit();
        } else {
            self.zoom_to(ctx, wanted);
        }
    }

    /// Zooms to a percentage of the video's real size, between a quarter of fit and the
    /// closest zoom, landing on fit when within half a percent of it.
    pub fn zoom_to(&mut self, ctx: &egui::Context, percent: f32) {
        let Some(video) = self.video_size() else {
            return;
        };
        let ppp = ctx.pixels_per_point();
        let fit = zoom::fit_percent(self.view, video, ppp);
        if (percent - fit).abs() <= 0.5 {
            self.fit();
            return;
        }
        let most = (zoom::MOST_ZOOM * 100.0).max(fit);
        let least = fit * zoom::LEAST_OF_FIT;
        let percent = percent.clamp(least, most);
        // From the same side of fit it keeps the part being looked at. Across fit it goes from
        // fit, because zooming through fit stops there.
        let now = self.zoom.percent(self.view, video, ppp);
        let base = if (now - fit) * (percent - fit) > 0.0 {
            self.zoom
        } else {
            Zoom::fit()
        };
        self.zoom = base.zoom_to_percent(self.view, video, percent, ppp);
    }

    /// Whether the picture is zoomed in past fit, so a drag pans it rather than a click playing.
    fn zoomed_in(&self) -> bool {
        self.video_size()
            .is_some_and(|video| self.zoom.is_zoomed_in(self.view, video))
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

    /// Whether the pill shows. Any movement of the mouse brings it, and it goes once the mouse
    /// has rested for a moment, a little longer while paused, unless the mouse rests on it, a
    /// flyout or menu is open, a slider is held, or a window is asking something.
    fn pill_shown(&self, ctx: &egui::Context) -> bool {
        let on_pill = ctx
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|at| self.pill_at.contains(at));
        let keep = self.seek_bar_held
            || self.volume_held
            || self.settings_open
            || self.confirm_delete
            || on_pill
            || self.flyout.is_open()
            || egui::Popup::is_any_open(ctx);
        let linger = if self.is_playing() {
            PILL_LINGER
        } else {
            PILL_LINGER_PAUSED
        };
        let since = self.last_pointer_move.elapsed();
        if since < linger {
            ctx.request_repaint_after(linger - since);
            return true;
        }
        keep
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
        // Escape closes an open flyout or menu first, before leaving full screen or the trimmer.
        if (self.flyout.is_open() || egui::Popup::is_any_open(ctx))
            && consume(Modifiers::NONE, Key::Escape)
        {
            self.flyout.close();
            egui::Popup::close_all(ctx);
            return;
        }
        // Playing, pausing or moving through the video closes a flyout, so it doesn't sit over
        // the picture being watched.
        let moves_playback = ctx.input(|input| {
            PLAYBACK_KEYS
                .iter()
                .any(|&key| input.key_pressed(key) && input.modifiers.is_none())
        });
        if moves_playback {
            self.flyout.close();
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
                self.go_to_end(false);
            }
            if consume(Modifiers::NONE, Key::End) {
                self.go_to_end(true);
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
        let full_screen_key =
            consume(Modifiers::NONE, Key::F) || consume(Modifiers::NONE, Key::F11);
        if full_screen_key && self.mini.is_none() {
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
            self.zoom_step(ctx, true);
        }
        if consume(Modifiers::COMMAND, Key::Minus) {
            self.zoom_step(ctx, false);
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
        ui.painter().rect_filled(area, 0.0, palette.bg_app);
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
            let reason = failed.reason.clone();
            stage_message(
                ui,
                palette,
                area,
                (&icons::CIRCLE_ALERT, palette.danger),
                "This file can't be played",
                &reason,
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
            stage_message(
                ui,
                palette,
                area,
                (&icons::FILM, palette.text_muted),
                "Nothing open",
                "Open a video or sound from File Explorer to play it here.",
            );
            return;
        };
        if file.is_sound() {
            let cover = file.cover.clone();
            let has_cover = cover.is_some();
            let picture = cover.or_else(|| self.lockup(ui.ctx()));
            if let Some(picture) = picture {
                let size = picture.size_vec2();
                let room = if has_cover {
                    area.shrink(24.0).size()
                } else {
                    let widest = LOCKUP_WIDTH.min(area.width() - 48.0).max(0.0);
                    vec2(widest, area.height())
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
        } else if response.clicked() && !self.zoomed_in() {
            self.toggle_playback();
        }
        if response.drag_started() {
            self.panning = self.zoomed_in();
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
                // The zoom's flyout shows the new level for a moment.
                self.flyout.show_for_a_moment(bars::Flyout::Zoom);
            }
        }
    }

    /// The BareClip logo and name, which stand in for a sound file's missing cover, in the
    /// colours for the theme showing.
    fn lockup(&mut self, ctx: &egui::Context) -> Option<TextureHandle> {
        let dark = ctx.theme() == egui::Theme::Dark;
        if self
            .lockup
            .as_ref()
            .is_none_or(|(made_for, _)| *made_for != dark)
        {
            let png = if dark { LOCKUP_DARK } else { LOCKUP_LIGHT };
            let image = eframe::icon_data::from_png_bytes(png).ok()?;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [image.width as usize, image.height as usize],
                &image.rgba,
            );
            let name = if dark {
                "viewer-lockup-dark"
            } else {
                "viewer-lockup-light"
            };
            let texture = ctx.load_texture(name, image, TextureOptions::LINEAR);
            self.lockup = Some((dark, texture));
        }
        self.lockup.as_ref().map(|(_, texture)| texture.clone())
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
                .map_err(|error| format!("The frame couldn't be saved. {error}"));
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
    fn saved_picture_note(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        area: Rect,
        top: &mut f32,
    ) {
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
        let words = format!("Saved {name}");
        match Note::new(&icons::CIRCLE_CHECK, palette.success, &words)
            .actions(&["Show in folder"])
            .closable()
            .show(ui, palette, area, top)
        {
            NotePress::Action(_) => show_in_folder(&path),
            NotePress::Close => self.saved_picture = None,
            NotePress::Nothing => {}
        }
    }

    /// The player's Settings, laid out as the editor's: tabs down the left, the open tab's page
    /// beside them, and the version and Done along the bottom.
    fn settings_window(&mut self, ctx: &egui::Context, palette: &Palette) {
        if !self.settings_open {
            self.default_player = None;
            return;
        }
        let back_in_front = ctx.input(|input| {
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::WindowFocused(true)))
        });
        if self.default_player.is_none() || back_in_front {
            self.default_player = Some((self.is_default_player)());
        }
        let is_default = self.default_player.unwrap_or(false);
        let mut close = false;
        let mut jumps = [self.jump(false), self.jump(true)];
        let mut tab = self.settings_tab;
        egui::Modal::new(egui::Id::unique("viewer_settings"))
            .frame(crate::editor::dialog_frame(ctx))
            .show(ctx, |ui| {
                let room = crate::editor::dialog_room(ctx);
                ui.set_width(components::SETTINGS_WIDTH.min(room.x));
                components::dialog_title(ui, &icons::SETTINGS, palette.accent, "Settings");
                ui.add_space(SETTINGS_GAP);
                // Only the tabs and their page scroll in a short window, so the title and Done
                // stay in sight.
                let height = (ctx.content_rect().height() - SETTINGS_FRAME_ROOM).max(120.0);
                // A scroll bar beside the page rather than over its right edge.
                ui.spacing_mut().scroll.floating = false;
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        components::settings_tabs(
                            ui,
                            palette,
                            &SETTINGS_TABS,
                            &mut tab,
                            SETTINGS_PAGE_HEIGHT,
                            |ui, tab| match tab {
                                SettingsTab::General => {
                                    general_settings(ui, palette, &mut jumps, is_default);
                                }
                                SettingsTab::Keys => key_settings(ui, palette),
                            },
                        );
                    });
                ui.add_space(12.0);
                components::dialog_footer(ui, palette, &text::version_line(), |ui| {
                    close = ui
                        .add(crate::editor::dialog_button(palette, "Done"))
                        .clicked();
                });
            });
        self.settings_tab = tab;
        for (forward, seconds) in [false, true].into_iter().zip(jumps) {
            if seconds != self.jump(forward) {
                self.set_jump(forward, seconds);
            }
        }
        let theme = ctx.options(|options| options.theme_preference).into();
        if theme != self.theme {
            self.theme = theme;
            self.save_settings();
        }
        if close {
            self.settings_open = false;
        }
    }

    /// A problem, as the first note over the picture, until it is closed or the file changes.
    fn show_notice(&mut self, ui: &mut egui::Ui, palette: &Palette, area: Rect, top: &mut f32) {
        let Some(notice) = self.notice.clone() else {
            return;
        };
        if Note::new(&icons::CIRCLE_ALERT, palette.danger, &notice)
            .closable()
            .show(ui, palette, area, top)
            == NotePress::Close
        {
            self.notice = None;
        }
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

/// The tabs of the player's Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsTab {
    General,
    Keys,
}

/// Each tab's symbol, label and name for screen readers.
const SETTINGS_TABS: [(SettingsTab, icons::Icon, &str, &str); 2] = [
    (
        SettingsTab::General,
        icons::SLIDERS_HORIZONTAL,
        "General",
        "General settings",
    ),
    (SettingsTab::Keys, icons::KEYBOARD, "Keys", "Keys settings"),
];

/// Settings' General tab: the jumps, the theme, and making
/// BareClip the default player, which shows a tick once it is.
fn general_settings(ui: &mut egui::Ui, palette: &Palette, jumps: &mut [u32; 2], is_default: bool) {
    components::settings_group(ui, palette, "Playing", |ui| {
        for (index, (label, name)) in [
            ("Jump back by", "Jump back"),
            ("Jump forward by", "Jump forward"),
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                components::settings_divider(ui, palette);
            }
            components::settings_row(ui, palette, (label, None), JUMP_DROPDOWN_WIDTH, |ui| {
                jump_dropdown(ui, palette, name, &mut jumps[index]);
            });
        }
    });
    ui.add_space(SETTINGS_GAP);
    components::settings_group(ui, palette, "Appearance", |ui| {
        components::settings_row(
            ui,
            palette,
            ("Theme", None),
            theme::THEME_PICKER_WIDTH,
            |ui| theme::theme_picker(ui, palette),
        );
    });
    ui.add_space(SETTINGS_GAP);
    components::settings_group(ui, palette, "Default player", |ui| {
        components::settings_row(
            ui,
            palette,
            (
                "Open videos and sound in BareClip",
                Some(if is_default {
                    DEFAULT_PLAYER_NOTE
                } else {
                    NOT_DEFAULT_PLAYER_NOTE
                }),
            ),
            MAKE_DEFAULT_WIDTH,
            |ui| {
                if is_default {
                    default_mark(ui, palette);
                    return;
                }
                let button = ActionButton::new(palette, Kind::Secondary, "Make default")
                    .small()
                    .width(MAKE_DEFAULT_WIDTH);
                if ui
                    .add(button)
                    .on_hover_text("Make BareClip your default player")
                    .clicked()
                {
                    crate::file_ops::open_default_apps();
                }
            },
        );
    });
}

/// In place of Make default once BareClip is the default: a tick and the word, with no box, so
/// it reads as a fact rather than something to press, in full colour so it is easy to read.
fn default_mark(ui: &mut egui::Ui, palette: &Palette) {
    const ICON: f32 = 14.0;
    const GAP: f32 = 6.0;
    let (rect, response) = ui.allocate_exact_size(
        vec2(MAKE_DEFAULT_WIDTH, DEFAULT_MARK_HEIGHT),
        Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, "Default"));
    let galley = ui.painter().layout_no_wrap(
        "Default".to_owned(),
        FontId::proportional(12.0),
        palette.text,
    );
    let width = ICON + GAP + galley.size().x;
    let left = rect.center().x - width / 2.0;
    icons::paint(
        ui.painter(),
        icons::square_at(pos2(left + ICON / 2.0, rect.center().y), ICON),
        &icons::CHECK,
        palette.accent,
    );
    let at = pos2(left + ICON + GAP, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(at, galley, palette.text);
}

/// A jump's choices, from 5 to 30 seconds, in a dropdown.
fn jump_dropdown(ui: &mut egui::Ui, palette: &Palette, label: &str, seconds: &mut u32) {
    let shown = format!("{seconds} s");
    let name = format!("{label}: {shown}");
    let button = ui.add(components::Dropdown::new(
        palette,
        &name,
        &shown,
        JUMP_DROPDOWN_WIDTH,
    ));
    let width = components::menu_width(&button);
    egui::Popup::menu(&button).show(|ui| {
        for choice in storage::JUMP_CHOICES {
            let label = format!("{choice} s");
            let item = MenuItem::choice(palette, &label, *seconds == choice).width(width);
            if ui.add(item).clicked() {
                *seconds = choice;
            }
        }
    });
}

/// Settings' Keys tab: what each key does.
fn key_settings(ui: &mut egui::Ui, palette: &Palette) {
    components::settings_group(ui, palette, "Keys", |ui| {
        for (index, (keys, what)) in KEY_LIST.iter().enumerate() {
            if index > 0 {
                components::settings_divider(ui, palette);
            }
            key_row(ui, palette, keys, what);
        }
    });
}

/// The keys, as Settings lists them.
const KEY_LIST: &[(&[&str], &str)] = &[
    (&["Space", "K"], "Play or pause"),
    (&["Left", "Right"], "Jump back or forward"),
    (&[",", "."], "Step one frame"),
    (&["Up", "Down"], "Volume"),
    (&["M"], "Mute"),
    (&["Home", "End"], "Start or end"),
    (&["PgUp", "PgDn"], "Previous or next file"),
    (&["0", "Ctrl+=", "Ctrl+-"], "Fit or zoom"),
    (&["F", "Esc"], "Full screen"),
];

/// A line of the key list: what it does on the left, and its keys as caps on the right.
fn key_row(ui: &mut egui::Ui, palette: &Palette, keys: &[&str], what: &str) {
    const PADDING: f32 = 12.0;
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), KEY_ROW_HEIGHT), Sense::hover());
    let painter = ui.painter();
    painter.text(
        pos2(rect.left() + PADDING, rect.center().y),
        Align2::LEFT_CENTER,
        what,
        FontId::proportional(13.0),
        palette.text,
    );
    let mut right = rect.right() - PADDING;
    for key in keys.iter().rev() {
        let galley =
            painter.layout_no_wrap((*key).to_owned(), FontId::proportional(11.5), palette.text);
        let cap = Rect::from_min_max(
            pos2(right - galley.size().x - 12.0, rect.center().y - 10.0),
            pos2(right, rect.center().y + 10.0),
        );
        painter.rect_filled(cap, 4.0, palette.raised);
        painter.rect_stroke(
            cap,
            4.0,
            Stroke::new(1.0, palette.border),
            egui::StrokeKind::Inside,
        );
        painter.galley(cap.center() - galley.size() / 2.0, galley, palette.text);
        right = cap.left() - 4.0;
    }
}

/// One of the watching speeds, keeping the voice's pitch.
fn watching_speed(index: usize) -> Speed {
    Speed {
        keep_pitch: true,
        ..Speed::default()
    }
    .with_factor(SPEEDS[index.min(SPEEDS.len() - 1)])
}

/// A symbol, a heading and a line under it, in the middle of the picture's space, which is the
/// window's own colour.
fn stage_message(
    ui: &egui::Ui,
    palette: &Palette,
    area: Rect,
    (icon, tint): (&icons::Icon, Color32),
    heading: &str,
    detail: &str,
) {
    let painter = ui.painter_at(area);
    let middle = area.center();
    icons::paint(
        &painter,
        icons::square_at(middle - vec2(0.0, 40.0), STAGE_SYMBOL),
        icon,
        tint,
    );
    painter.text(
        middle,
        Align2::CENTER_CENTER,
        heading,
        fonts::strong(18.0),
        palette.text,
    );
    let detail = painter.layout(
        detail.to_owned(),
        FontId::proportional(13.0),
        palette.text_muted,
        (area.width() - 48.0).clamp(80.0, 460.0),
    );
    let at = pos2(middle.x - detail.size().x / 2.0, middle.y + 16.0);
    painter.galley(at, detail, palette.text_muted);
}

/// Whether a window this wide has room for the info card beside the picture. Narrower, the card
/// lies over the picture.
pub fn info_narrows_picture(width: f32) -> bool {
    width > info::CARD_WIDTH * 2.0
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
            "{} \u{d7} {}",
            file.info.video.width, file.info.video.height
        ));
    }
    details.push(text::file_size(file.info.size_bytes));
    details
}

mod bars;
mod browse;
mod info;
mod mini;
mod note;
mod pill;
#[cfg(test)]
mod tests;
mod trim;
mod zoom;
