//! The trimming screen: everything between opening a video and exporting it.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

use bareclip_core::{
    cut::CutList,
    splice::{CutMode, FrameIndex},
    time::{Ticks, TimeBase, format_timecode},
};
use bareclip_media::{
    ExportReport, ExportRequest, MediaError, MediaInfo, export, plan_export, probe, scan_frames,
};
use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, FontFamily, FontId, Frame, Key, Layout, Margin,
    Modifiers, RichText, Sense, Stroke, TextureHandle, TextureOptions, pos2, vec2,
};

use crate::{
    fonts,
    player::Player,
    text,
    theme::{self, Palette},
    timeline::{self, TimelineDrag},
};

const VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mkv", "mov", "webm", "avi", "ts"];
const LOGO_DARK: &[u8] = include_bytes!("../../../assets/brand/png/mark-dark-256.png");
const LOGO_LIGHT: &[u8] = include_bytes!("../../../assets/brand/png/mark-light-256.png");
const SHIFT_STEP_SECONDS: f64 = 1.0;
const BACKGROUND_TICK: Duration = Duration::from_millis(50);

/// The frame table being read in the background.
struct ScanTask {
    result: Receiver<Result<FrameIndex, MediaError>>,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
}

impl Drop for ScanTask {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct ExportTask {
    result: Receiver<Result<ExportReport, MediaError>>,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    output: PathBuf,
}

enum ExportState {
    Closed,
    Choosing,
    Running(ExportTask),
    Done {
        report: ExportReport,
        output: PathBuf,
    },
    Failed(String),
}

struct Loaded {
    info: MediaInfo,
    player: Player,
    cuts: CutList,
    index: Option<Arc<FrameIndex>>,
    clean_points: Vec<Ticks>,
    scan: Option<ScanTask>,
    picture: Option<TextureHandle>,
    drag: TimelineDrag,
}

impl Loaded {
    fn time_base(&self) -> TimeBase {
        self.info.video.time_base
    }

    /// The position a mark should use: the start of the frame on screen.
    fn frame_position(&self) -> Ticks {
        let position = self.player.position();
        let Some(index) = &self.index else {
            return position;
        };
        let time_base = self.time_base();
        index
            .frame_at(position.to_pts(time_base))
            .map_or(position, |pts| Ticks::from_pts(pts, time_base))
    }

    fn step_frame(&mut self, forward: bool) {
        self.player.pause();
        let position = self.frame_position();
        let time_base = self.time_base();
        let pts = position.to_pts(time_base);
        let target = self.index.as_ref().and_then(|index| {
            if forward {
                index.next_frame_after(pts)
            } else {
                index.previous_frame_before(pts)
            }
        });
        if let Some(target) = target {
            self.player.seek(Ticks::from_pts(target, time_base));
        }
    }

    fn jump(&mut self, seconds: f64) {
        let target = Ticks(self.player.position().0 + Ticks::from_seconds(seconds).0);
        self.player.seek(target);
    }
}

pub struct Editor {
    loaded: Option<Loaded>,
    notice: Option<String>,
    export_state: ExportState,
    mode: CutMode,
    logo: Option<(bool, TextureHandle)>,
    loaded_fallbacks: Vec<&'static str>,
    use_sound: bool,
}

impl Editor {
    pub fn new(use_sound: bool) -> Self {
        Self {
            loaded: None,
            notice: None,
            export_state: ExportState::Closed,
            mode: CutMode::FrameExact,
            logo: None,
            loaded_fallbacks: Vec::new(),
            use_sound,
        }
    }

    /// Opens a video, replacing any open one. The current video stays if this fails.
    pub fn open(&mut self, ctx: &egui::Context, path: &Path) {
        match self.load(ctx, path) {
            Ok(loaded) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned());
                if let Some(name) = name {
                    fonts::load_fallbacks_for(ctx, &name, &mut self.loaded_fallbacks);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                        "{name} - BareClip"
                    )));
                }
                self.loaded = Some(loaded);
                self.notice = None;
                self.export_state = ExportState::Closed;
            }
            Err(error) => self.notice = Some(error.to_string()),
        }
    }

    fn load(&self, ctx: &egui::Context, path: &Path) -> Result<Loaded, MediaError> {
        let info = probe(path)?;
        let repaint = ctx.clone();
        let mut player = Player::open(
            &info,
            self.use_sound,
            Arc::new(move || repaint.request_repaint()),
        )
        .map_err(|error| MediaError::Io(std::io::Error::other(error)))?;
        player.seek(Ticks(0));

        let (sender, result) = mpsc::channel();
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let scan_path = path.to_owned();
        let scan_progress = progress.clone();
        let scan_cancel = cancel.clone();
        let repaint = ctx.clone();
        thread::spawn(move || {
            let outcome = scan_frames(&scan_path, &scan_cancel, |fraction| {
                scan_progress.store(fraction.to_bits(), Ordering::Relaxed);
                repaint.request_repaint();
            });
            let _ = sender.send(outcome);
            repaint.request_repaint();
        });

        Ok(Loaded {
            cuts: CutList::new(info.duration),
            info,
            player,
            index: None,
            clean_points: Vec::new(),
            scan: Some(ScanTask {
                result,
                progress,
                cancel,
            }),
            picture: None,
            drag: TimelineDrag::None,
        })
    }

    fn pick_file(&mut self, ctx: &egui::Context) {
        let picked = rfd::FileDialog::new()
            .set_title("Open a video")
            .add_filter("Video files", &VIDEO_EXTENSIONS)
            .add_filter("All files", &["*"])
            .pick_file();
        if let Some(path) = picked {
            self.open(ctx, &path);
        }
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        if let Some(loaded) = &mut self.loaded {
            if let Some(scan) = &loaded.scan {
                match scan.result.try_recv() {
                    Ok(Ok(index)) => {
                        let time_base = loaded.info.video.time_base;
                        loaded.clean_points = index
                            .clean_points()
                            .into_iter()
                            .map(|pts| Ticks::from_pts(pts, time_base))
                            .collect();
                        loaded.index = Some(Arc::new(index));
                        loaded.scan = None;
                    }
                    Ok(Err(error)) => {
                        self.notice = Some(error.to_string());
                        loaded.scan = None;
                    }
                    Err(_) => ctx.request_repaint_after(BACKGROUND_TICK),
                }
            }

            if let Some(frame) = loaded.player.update() {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width as usize, frame.height as usize],
                    &frame.rgba,
                );
                match &mut loaded.picture {
                    Some(picture) => picture.set(image, TextureOptions::LINEAR),
                    None => {
                        loaded.picture =
                            Some(ctx.load_texture("video", image, TextureOptions::LINEAR));
                    }
                }
            }
            if let Some(error) = loaded.player.take_error() {
                self.notice = Some(error);
            }
            if loaded.player.is_playing() {
                ctx.request_repaint();
            }
        }

        let finished = match &self.export_state {
            ExportState::Running(task) => match task.result.try_recv() {
                Ok(outcome) => Some((outcome, task.output.clone())),
                Err(_) => {
                    ctx.request_repaint_after(BACKGROUND_TICK);
                    None
                }
            },
            _ => None,
        };
        if let Some((outcome, output)) = finished {
            self.export_state = match outcome {
                Ok(report) => ExportState::Done { report, output },
                Err(MediaError::Cancelled) => ExportState::Choosing,
                Err(error) => ExportState::Failed(error.to_string()),
            };
        }
    }

    fn start_export(&mut self, ctx: &egui::Context) {
        let Some(loaded) = &self.loaded else {
            return;
        };
        let suggested = text::default_output_path(&loaded.info.path);
        let mut dialog = rfd::FileDialog::new().set_title("Export video as");
        if let Some(folder) = suggested.parent() {
            dialog = dialog.set_directory(folder);
        }
        if let Some(name) = suggested.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        if let Some(extension) = suggested.extension() {
            let extension = extension.to_string_lossy().into_owned();
            dialog = dialog.add_filter(format!("{} video", extension.to_uppercase()), &[extension]);
        }
        if let Some(output) = dialog.save_file() {
            self.start_export_to(ctx, output);
        }
    }

    fn start_export_to(&mut self, ctx: &egui::Context, output: PathBuf) {
        let Some(loaded) = &self.loaded else {
            return;
        };
        let Some(index) = loaded.index.clone() else {
            return;
        };
        let request = ExportRequest {
            input: loaded.info.path.clone(),
            output: output.clone(),
            ranges: loaded.cuts.export_ranges(),
            mode: self.mode,
        };
        let (sender, result) = mpsc::channel();
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_progress = progress.clone();
        let worker_cancel = cancel.clone();
        let repaint = ctx.clone();
        thread::spawn(move || {
            let outcome = export(&request, &index, &worker_cancel, &|fraction| {
                worker_progress.store(fraction.to_bits(), Ordering::Relaxed);
            });
            let _ = sender.send(outcome);
            repaint.request_repaint();
        });
        self.export_state = ExportState::Running(ExportTask {
            result,
            progress,
            cancel,
            output,
        });
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let dialog_open = !matches!(self.export_state, ExportState::Closed);
        let consume = |modifiers, key| ctx.input_mut(|input| input.consume_key(modifiers, key));

        if consume(Modifiers::COMMAND, Key::O) && !dialog_open {
            self.pick_file(ctx);
        }
        if dialog_open {
            return;
        }
        if consume(Modifiers::COMMAND, Key::E) && self.can_export() {
            self.export_state = ExportState::Choosing;
        }
        let Some(loaded) = &mut self.loaded else {
            return;
        };

        if consume(Modifiers::NONE, Key::Space) {
            loaded.player.toggle();
        }
        if consume(Modifiers::NONE, Key::I) {
            loaded.cuts.mark_in(loaded.frame_position());
        }
        if consume(Modifiers::NONE, Key::O) {
            let position = loaded.frame_position();
            if !loaded.cuts.mark_out(position) {
                self.notice = Some("The end of a section has to come after its start.".to_owned());
            }
        }
        if consume(Modifiers::SHIFT, Key::ArrowLeft) {
            loaded.jump(-SHIFT_STEP_SECONDS);
        }
        if consume(Modifiers::SHIFT, Key::ArrowRight) {
            loaded.jump(SHIFT_STEP_SECONDS);
        }
        if consume(Modifiers::NONE, Key::ArrowLeft) {
            loaded.step_frame(false);
        }
        if consume(Modifiers::NONE, Key::ArrowRight) {
            loaded.step_frame(true);
        }
        if consume(Modifiers::NONE, Key::Home) {
            loaded.player.seek(Ticks(0));
        }
        if consume(Modifiers::NONE, Key::End) {
            let duration = loaded.player.duration();
            loaded.player.seek(duration);
        }
    }

    fn can_export(&self) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|loaded| !loaded.cuts.export_ranges().is_empty())
    }

    fn logo(&mut self, ctx: &egui::Context, dark: bool) -> Option<TextureHandle> {
        if self
            .logo
            .as_ref()
            .is_none_or(|(is_dark, _)| *is_dark != dark)
        {
            let bytes = if dark { LOGO_DARK } else { LOGO_LIGHT };
            let icon = eframe::icon_data::from_png_bytes(bytes).ok()?;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            self.logo = Some((
                dark,
                ctx.load_texture("logo", image, TextureOptions::LINEAR),
            ));
        }
        self.logo.as_ref().map(|(_, texture)| texture.clone())
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let palette = theme::palette(&ctx);

        self.poll_background(&ctx);
        self.handle_shortcuts(&ctx);
        self.take_dropped_file(&ctx);

        self.top_bar(ui, palette);
        if self.loaded.is_some() {
            self.bottom_panel(ui, palette);
            self.side_panel(ui, palette);
        }
        self.stage(ui, palette);
        self.export_dialog(&ctx, palette);
        drop_overlay(&ctx, palette);
    }

    fn take_dropped_file(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .first()
                .map(|file| file.path().to_owned())
        });
        if let Some(path) = dropped {
            self.open(ctx, &path);
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new()
            .fill(palette.surface)
            .inner_margin(Margin::symmetric(14, 8))
            .stroke(Stroke::new(1.0, palette.border));
        egui::Panel::top("top_bar")
            .exact_size(48.0)
            .frame(frame)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let title = self.loaded.as_ref().map_or_else(
                        || "BareClip".to_owned(),
                        |loaded| {
                            loaded.info.path.file_name().map_or_else(
                                || "Untitled".to_owned(),
                                |name| name.to_string_lossy().into_owned(),
                            )
                        },
                    );
                    ui.label(RichText::new(title).font(fonts::strong(14.0)));
                    if ui
                        .button("Open")
                        .on_hover_text("Open a video (Ctrl+O)")
                        .clicked()
                    {
                        let ctx = ui.ctx().clone();
                        self.pick_file(&ctx);
                    }
                    if let Some(notice) = self.notice.clone() {
                        ui.add_space(8.0);
                        ui.colored_label(palette.danger, notice);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let can_export = self.can_export();
                        let export = accent_button(ui, palette, "Export", can_export);
                        let export = if can_export {
                            export.on_hover_text("Export the marked sections (Ctrl+E)")
                        } else {
                            export.on_disabled_hover_text("Mark a section to keep first")
                        };
                        if export.clicked() {
                            self.export_state = ExportState::Choosing;
                        }
                        ui.menu_button("Theme", |ui| {
                            theme::theme_picker(ui);
                        });
                        if let Some(scan) =
                            self.loaded.as_ref().and_then(|loaded| loaded.scan.as_ref())
                        {
                            let fraction =
                                f32::from_bits(scan.progress.load(Ordering::Relaxed)) * 100.0;
                            ui.colored_label(
                                palette.text_muted,
                                format!("Reading frames {fraction:.0}%"),
                            );
                        }
                    });
                });
            });
    }

    fn bottom_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new()
            .fill(palette.surface)
            .inner_margin(Margin::symmetric(8, 8))
            .stroke(Stroke::new(1.0, palette.border));
        egui::Panel::bottom("timeline")
            .exact_size(164.0)
            .frame(frame)
            .show(ui, |ui| {
                let Some(loaded) = &mut self.loaded else {
                    return;
                };
                transport(ui, palette, loaded);
                ui.add_space(6.0);

                let position = loaded.player.position();
                let duration = loaded.player.duration();
                let output = timeline::timeline(
                    ui,
                    palette,
                    &mut loaded.cuts,
                    position,
                    duration,
                    &loaded.clean_points,
                    &mut loaded.drag,
                );
                if let Some(seek) = output.seek {
                    loaded.player.seek(seek);
                }
            });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new()
            .fill(palette.surface)
            .inner_margin(Margin::same(14))
            .stroke(Stroke::new(1.0, palette.border));
        egui::Panel::right("details")
            .exact_size(290.0)
            .frame(frame)
            .show(ui, |ui| {
                let Some(loaded) = &mut self.loaded else {
                    return;
                };
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        sections_list(ui, palette, loaded);
                        ui.add_space(18.0);
                        details(ui, palette, &loaded.info);
                        if !loaded.info.audio.is_empty() && !loaded.player.has_sound() {
                            ui.add_space(8.0);
                            ui.colored_label(
                                palette.text_muted,
                                "Sound can't play on this computer, so the preview is silent. Exports keep every audio track.",
                            );
                        }
                    });
            });
    }

    fn stage(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new().fill(palette.stage);
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let rect = ui.available_rect_before_wrap();
            if let Some(loaded) = &self.loaded {
                if let Some(picture) = &loaded.picture {
                    let size = picture.size_vec2();
                    let scale = (rect.width() / size.x).min(rect.height() / size.y);
                    let shown = egui::Rect::from_center_size(rect.center(), size * scale);
                    ui.painter().image(
                        picture.id(),
                        shown,
                        egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                return;
            }

            let ctx = ui.ctx().clone();
            let logo = self.logo(&ctx, true);
            let mut open_clicked = false;
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((rect.height() - 280.0).max(0.0) / 2.0);
                    if let Some(logo) = logo {
                        ui.add(egui::Image::new(&logo).fit_to_exact_size(vec2(110.0, 128.0)));
                        ui.add_space(20.0);
                    }
                    ui.label(
                        RichText::new("Open a video to start trimming")
                            .font(fonts::strong(18.0))
                            .color(Color32::from_rgb(0xF1, 0xEE, 0xE9)),
                    );
                    ui.add_space(14.0);
                    open_clicked = accent_button(ui, palette, "Open video", true).clicked();
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Or drop a file anywhere in this window")
                            .color(Color32::from_rgb(0xAB, 0xA6, 0x9F)),
                    );
                });
            });
            if open_clicked {
                self.pick_file(&ctx);
            }
        });
    }

    fn export_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        if matches!(self.export_state, ExportState::Closed) {
            return;
        }
        let mut next_state = None;
        let mut begin_export = false;

        let frame = Frame::popup(&ctx.global_style()).inner_margin(Margin::same(20));
        egui::Modal::new(egui::Id::unique("export_dialog")).frame(frame).show(ctx, |ui| {
            ui.set_width(440.0);
            match &self.export_state {
                ExportState::Choosing => {
                    ui.label(RichText::new("Export").font(fonts::strong(18.0)));
                    ui.add_space(12.0);
                    ui.radio_value(&mut self.mode, CutMode::FrameExact, "Frame-exact");
                    ui.indent("exact", |ui| {
                        ui.colored_label(
                            palette.text_muted,
                            "Cuts land exactly where you marked them. Short parts around each cut are re-encoded.",
                        );
                    });
                    ui.add_space(8.0);
                    ui.radio_value(&mut self.mode, CutMode::Keyframe, "Fastest");
                    ui.indent("fast", |ui| {
                        ui.colored_label(
                            palette.text_muted,
                            "Nothing is re-encoded, but each cut moves outward to the nearest keyframe, keeping a little extra.",
                        );
                    });
                    ui.add_space(14.0);

                    let ready = self.loaded.as_ref().is_some_and(|loaded| loaded.index.is_some());
                    if let Some(summary) = self.plan_description() {
                        ui.label(summary);
                    } else {
                        ui.colored_label(palette.text_muted, "Still reading the video's frames.");
                    }
                    ui.add_space(16.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if accent_button(ui, palette, "Choose file and export", ready).clicked() {
                            begin_export = true;
                        }
                        if ui.button("Cancel").clicked() {
                            next_state = Some(ExportState::Closed);
                        }
                    });
                }
                ExportState::Running(task) => {
                    ui.label(RichText::new("Exporting").font(fonts::strong(18.0)));
                    ui.add_space(12.0);
                    let fraction = f32::from_bits(task.progress.load(Ordering::Relaxed));
                    ui.add(egui::ProgressBar::new(fraction).show_percentage());
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            task.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                }
                ExportState::Done { report, output } => {
                    ui.label(RichText::new("Export finished").font(fonts::strong(18.0)));
                    ui.add_space(12.0);
                    for line in text::export_summary(report) {
                        ui.label(line);
                    }
                    ui.add_space(6.0);
                    ui.colored_label(palette.text_muted, output.display().to_string());
                    ui.add_space(16.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if accent_button(ui, palette, "Done", true).clicked() {
                            next_state = Some(ExportState::Closed);
                        }
                        if ui.button("Show in folder").clicked() {
                            show_in_folder(output);
                        }
                    });
                }
                ExportState::Failed(error) => {
                    ui.label(RichText::new("Export failed").font(fonts::strong(18.0)));
                    ui.add_space(12.0);
                    ui.colored_label(palette.danger, error);
                    ui.add_space(16.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            next_state = Some(ExportState::Choosing);
                        }
                    });
                }
                ExportState::Closed => {}
            }
        });

        if begin_export {
            self.start_export(ctx);
        }
        if let Some(state) = next_state {
            self.export_state = state;
        }
    }

    fn plan_description(&self) -> Option<String> {
        let loaded = self.loaded.as_ref()?;
        let index = loaded.index.as_ref()?;
        let plan = plan_export(
            index,
            &loaded.cuts.export_ranges(),
            loaded.time_base(),
            self.mode,
        );
        Some(text::plan_summary(plan.copied_frames, plan.encoded_frames))
    }
}

fn accent_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    enabled: bool,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        Button::new(RichText::new(label).color(palette.accent_text).strong())
            .fill(palette.accent)
            .corner_radius(CornerRadius::same(5))
            .min_size(vec2(84.0, 30.0)),
    )
}

/// Draws the play or pause symbol, since egui's fonts may not carry these glyphs.
fn play_button(ui: &mut egui::Ui, palette: &Palette, playing: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    let fill = if response.hovered() {
        palette.accent_hover
    } else {
        palette.accent
    };
    painter.circle_filled(center, 16.0, fill);
    if playing {
        for offset in [-4.0, 4.0] {
            painter.rect_filled(
                egui::Rect::from_center_size(center + vec2(offset, 0.0), vec2(4.0, 14.0)),
                1.0,
                palette.accent_text,
            );
        }
    } else {
        painter.add(egui::Shape::convex_polygon(
            vec![
                center + vec2(-5.0, -8.0),
                center + vec2(8.0, 0.0),
                center + vec2(-5.0, 8.0),
            ],
            palette.accent_text,
            Stroke::NONE,
        ));
    }
    response
}

fn transport(ui: &mut egui::Ui, palette: &Palette, loaded: &mut Loaded) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let position = loaded.player.position();
        ui.label(RichText::new(format_timecode(position)).font(fonts::strong(15.0)));
        ui.colored_label(
            palette.text_muted,
            format!("/ {}", format_timecode(loaded.player.duration())),
        );
        ui.add_space(14.0);

        if play_button(ui, palette, loaded.player.is_playing())
            .on_hover_text("Play or pause (Space)")
            .clicked()
        {
            loaded.player.toggle();
        }
        if ui
            .button("Back")
            .on_hover_text("One frame back (Left arrow)")
            .clicked()
        {
            loaded.step_frame(false);
        }
        if ui
            .button("Next")
            .on_hover_text("One frame forward (Right arrow)")
            .clicked()
        {
            loaded.step_frame(true);
        }

        ui.add_space(14.0);
        if ui
            .button("Mark in")
            .on_hover_text("Start a section here (I)")
            .clicked()
        {
            loaded.cuts.mark_in(loaded.frame_position());
        }
        if ui
            .button("Mark out")
            .on_hover_text("End the section here (O)")
            .clicked()
        {
            let position = loaded.frame_position();
            loaded.cuts.mark_out(position);
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(8.0);
            let kept = loaded.cuts.kept_duration();
            let label = if kept.0 > 0 {
                format!("Keeping {}", text::short_duration(kept))
            } else {
                "Nothing marked yet".to_owned()
            };
            ui.colored_label(palette.text_muted, label);
        });
    });
}

fn sections_list(ui: &mut egui::Ui, palette: &Palette, loaded: &mut Loaded) {
    ui.label(RichText::new("Sections to keep").font(fonts::strong(14.0)));
    ui.add_space(8.0);

    let ranges = loaded.cuts.ranges().to_vec();
    let mut remove = None;
    let mut jump = None;
    for (position, range) in ranges.iter().enumerate() {
        ui.horizontal(|ui| {
            let label = format!(
                "{}  {}",
                format_timecode(range.start),
                text::short_duration(range.duration())
            );
            if ui.link(label).on_hover_text("Go to this section").clicked() {
                jump = Some(range.start);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Remove").clicked() {
                    remove = Some(position);
                }
            });
        });
    }
    if let Some(position) = remove {
        loaded.cuts.remove(position);
    }
    if let Some(time) = jump {
        loaded.player.seek(time);
    }

    if let Some(start) = loaded.cuts.pending_in() {
        ui.colored_label(
            palette.text_muted,
            format!(
                "{} to the end, unless you mark an out point",
                format_timecode(start)
            ),
        );
    }
    if ranges.is_empty() && loaded.cuts.pending_in().is_none() {
        ui.colored_label(
            palette.text_muted,
            "Press I where a section should start and O where it should end. Repeat to keep several sections.",
        );
    }
    if !ranges.is_empty() {
        ui.add_space(6.0);
        if ui.small_button("Clear all").clicked() {
            loaded.cuts.clear();
        }
    }
}

fn details(ui: &mut egui::Ui, palette: &Palette, info: &MediaInfo) {
    ui.label(RichText::new("Details").font(fonts::strong(14.0)));
    ui.add_space(8.0);
    ui.colored_label(palette.text_muted, text::video_summary(info));
    ui.colored_label(palette.text_muted, text::file_size(info.size_bytes));
    if info.audio.is_empty() {
        ui.colored_label(palette.text_muted, "No audio");
    }
    for (position, track) in info.audio.iter().enumerate() {
        ui.colored_label(
            palette.text_muted,
            text::audio_track_summary(position, track),
        );
    }
    if info.video.is_hdr {
        ui.add_space(8.0);
        ui.colored_label(
            palette.danger,
            "This video is HDR. The preview shows it without HDR, and re-encoded parts may look different. The Fastest export keeps it unchanged.",
        );
    }
}

fn drop_overlay(ctx: &egui::Context, palette: &Palette) {
    if ctx.input(|input| input.raw.hovered_files.is_empty()) {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("drop_overlay"),
    ));
    let screen = ctx.content_rect();
    painter.rect_filled(screen, 0.0, palette.stage.gamma_multiply(0.85));
    painter.text(
        screen.center(),
        egui::Align2::CENTER_CENTER,
        "Drop to open",
        FontId::new(24.0, FontFamily::Proportional),
        Color32::from_rgb(0xF1, 0xEE, 0xE9),
    );
}

fn show_in_folder(path: &Path) {
    let _ = std::process::Command::new("explorer")
        .arg("/select,")
        .arg(path)
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{process::Command, sync::OnceLock, time::Instant};

    fn test_clip() -> &'static Path {
        static CLIP: OnceLock<PathBuf> = OnceLock::new();
        CLIP.get_or_init(|| {
            let folder = std::env::temp_dir().join("bareclip-app-tests");
            std::fs::create_dir_all(&folder).expect("the folder can be created");
            let path = folder.join(format!("editor-{}.mp4", std::process::id()));
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
                .arg("testsrc2=s=320x180:r=30:d=4,format=yuv420p")
                .args(["-c:v", "libopenh264", "-b:v", "2M", "-g", "30"])
                .arg(&path)
                .status()
                .expect("ffmpeg is on PATH");
            assert!(status.success());
            path
        })
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        theme::install(&ctx);
        ctx
    }

    fn run_frame(
        ctx: &egui::Context,
        editor: &mut Editor,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                vec2(1280.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| editor.show(ui));
        output.textures_delta.clear();
        output
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

    /// An editor with the test clip open and its frames read.
    fn opened(ctx: &egui::Context) -> Editor {
        let mut editor = Editor::new(false);
        run_frame(ctx, &mut editor, vec![]);
        editor.open(ctx, test_clip());
        let started = Instant::now();
        while editor
            .loaded
            .as_ref()
            .is_some_and(|loaded| loaded.index.is_none())
        {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "reading frames took too long"
            );
            run_frame(ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(5));
        }
        editor
    }

    fn seek_and_settle(ctx: &egui::Context, editor: &mut Editor, seconds: f64) {
        let loaded = editor.loaded.as_mut().expect("a video is open");
        loaded.player.seek(Ticks::from_seconds(seconds));
        run_frame(ctx, editor, vec![]);
    }

    #[test]
    fn the_empty_screen_draws_without_a_video() {
        let ctx = context();
        let mut editor = Editor::new(false);

        for _ in 0..2 {
            run_frame(&ctx, &mut editor, vec![]);
        }

        assert!(editor.loaded.is_none());
        assert!(!editor.can_export());
    }

    #[test]
    fn opening_a_video_reads_its_details_and_frames() {
        let ctx = context();
        let editor = opened(&ctx);
        let loaded = editor.loaded.as_ref().expect("a video is open");

        assert_eq!(loaded.info.video.width, 320);
        assert_eq!(
            loaded
                .index
                .as_ref()
                .expect("frames are read")
                .frames()
                .len(),
            120
        );
        assert_eq!(loaded.clean_points.len(), 4);
    }

    #[test]
    fn a_broken_file_keeps_the_current_video_and_explains() {
        let ctx = context();
        let mut editor = opened(&ctx);
        let broken = std::env::temp_dir().join("bareclip-not-a-video.mp4");
        std::fs::write(&broken, "not a video").expect("the file can be written");

        editor.open(&ctx, &broken);

        assert!(editor.loaded.is_some());
        assert!(editor.notice.is_some());
    }

    #[test]
    fn i_and_o_mark_a_section() {
        let ctx = context();
        let mut editor = opened(&ctx);
        assert!(!editor.can_export());

        seek_and_settle(&ctx, &mut editor, 1.0);
        run_frame(&ctx, &mut editor, press(Key::I));
        seek_and_settle(&ctx, &mut editor, 2.0);
        run_frame(&ctx, &mut editor, press(Key::O));

        let loaded = editor.loaded.as_ref().expect("a video is open");
        assert_eq!(loaded.cuts.ranges().len(), 1);
        assert!((loaded.cuts.ranges()[0].start.as_seconds() - 1.0).abs() < 0.04);
        assert!((loaded.cuts.ranges()[0].end.as_seconds() - 2.0).abs() < 0.04);
        assert!(editor.can_export());
    }

    #[test]
    fn an_out_before_the_in_explains_the_problem() {
        let ctx = context();
        let mut editor = opened(&ctx);

        seek_and_settle(&ctx, &mut editor, 2.0);
        run_frame(&ctx, &mut editor, press(Key::I));
        seek_and_settle(&ctx, &mut editor, 1.0);
        run_frame(&ctx, &mut editor, press(Key::O));

        assert!(
            editor
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("after its start"))
        );
        assert!(
            editor
                .loaded
                .as_ref()
                .expect("a video is open")
                .cuts
                .ranges()
                .is_empty()
        );
    }

    #[test]
    fn arrow_keys_step_one_frame_exactly() {
        let ctx = context();
        let mut editor = opened(&ctx);

        seek_and_settle(&ctx, &mut editor, 1.0);
        let before = editor
            .loaded
            .as_ref()
            .expect("a video is open")
            .frame_position();
        run_frame(&ctx, &mut editor, press(Key::ArrowRight));
        let after = editor
            .loaded
            .as_ref()
            .expect("a video is open")
            .player
            .position();

        let gap = Ticks(after.0 - before.0).as_seconds();
        assert!((gap - 1.0 / 30.0).abs() < 0.002, "stepped {gap}s");

        run_frame(&ctx, &mut editor, press(Key::ArrowLeft));
        let back = editor
            .loaded
            .as_ref()
            .expect("a video is open")
            .player
            .position();
        assert!(Ticks(back.0 - before.0).as_seconds().abs() < 0.002);
    }

    #[test]
    fn space_toggles_playback() {
        let ctx = context();
        let mut editor = opened(&ctx);

        run_frame(&ctx, &mut editor, press(Key::Space));
        assert!(
            editor
                .loaded
                .as_ref()
                .expect("a video is open")
                .player
                .is_playing()
        );

        run_frame(&ctx, &mut editor, press(Key::Space));
        assert!(
            !editor
                .loaded
                .as_ref()
                .expect("a video is open")
                .player
                .is_playing()
        );
    }

    #[test]
    fn shortcuts_are_ignored_while_the_export_dialog_is_open() {
        let ctx = context();
        let mut editor = opened(&ctx);
        editor.export_state = ExportState::Choosing;

        run_frame(&ctx, &mut editor, press(Key::Space));
        run_frame(&ctx, &mut editor, press(Key::I));

        let loaded = editor.loaded.as_ref().expect("a video is open");
        assert!(!loaded.player.is_playing());
        assert_eq!(loaded.cuts.pending_in(), None);
    }

    #[test]
    fn the_dialog_describes_what_the_export_will_do() {
        let ctx = context();
        let mut editor = opened(&ctx);
        {
            let loaded = editor.loaded.as_mut().expect("a video is open");
            loaded.cuts.mark_in(Ticks::from_seconds(0.5));
            loaded.cuts.mark_out(Ticks::from_seconds(2.5));
        }

        let exact = editor.plan_description().expect("the frames are read");
        editor.mode = CutMode::Keyframe;
        let fast = editor.plan_description().expect("the frames are read");

        assert!(exact.contains("30 frames are copied"), "{exact}");
        assert!(exact.contains("30 around the cuts"), "{exact}");
        assert!(fast.contains("All 90 frames are copied"), "{fast}");
    }

    /// Runs frames until the export finishes.
    fn wait_for_export(ctx: &egui::Context, editor: &mut Editor) {
        let started = Instant::now();
        while matches!(editor.export_state, ExportState::Running(_)) {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "the export took too long"
            );
            run_frame(ctx, editor, vec![]);
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn marked_editor(ctx: &egui::Context) -> Editor {
        let mut editor = opened(ctx);
        let loaded = editor.loaded.as_mut().expect("a video is open");
        loaded.cuts.mark_in(Ticks::from_seconds(0.5));
        loaded.cuts.mark_out(Ticks::from_seconds(2.5));
        editor
    }

    #[test]
    fn exporting_from_the_screen_writes_the_kept_sections() {
        let ctx = context();
        let mut editor = marked_editor(&ctx);
        let output =
            std::env::temp_dir().join(format!("bareclip-ui-export-{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&output);

        editor.start_export_to(&ctx, output.clone());
        wait_for_export(&ctx, &mut editor);

        let ExportState::Done {
            report,
            output: written,
        } = &editor.export_state
        else {
            panic!("the export did not finish");
        };
        assert_eq!(written, &output);
        assert_eq!(report.copied_frames, 30);
        assert_eq!(report.encoded_frames, 30);
        let info = probe(&output).expect("the export can be read");
        assert!((info.duration.as_seconds() - 2.0).abs() < 0.1);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn a_failed_export_explains_itself_and_can_be_retried() {
        let ctx = context();
        let mut editor = marked_editor(&ctx);
        let output = std::env::temp_dir()
            .join("bareclip-no-such-folder")
            .join("out.mp4");

        editor.start_export_to(&ctx, output);
        wait_for_export(&ctx, &mut editor);

        assert!(matches!(editor.export_state, ExportState::Failed(_)));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.loaded.is_some());
    }

    #[test]
    fn exporting_over_the_original_is_refused() {
        let ctx = context();
        let mut editor = marked_editor(&ctx);
        let before = std::fs::metadata(test_clip())
            .expect("the clip exists")
            .len();

        editor.start_export_to(&ctx, test_clip().to_owned());
        wait_for_export(&ctx, &mut editor);

        let ExportState::Failed(message) = &editor.export_state else {
            panic!("the export should have been refused");
        };
        assert!(message.contains("original stays untouched"), "{message}");
        assert_eq!(
            std::fs::metadata(test_clip())
                .expect("the clip exists")
                .len(),
            before
        );
    }

    #[test]
    fn opening_another_video_starts_fresh() {
        let ctx = context();
        let mut editor = opened(&ctx);
        editor
            .loaded
            .as_mut()
            .expect("a video is open")
            .cuts
            .mark_out(Ticks::from_seconds(1.0));

        editor.open(&ctx, test_clip());

        assert!(
            editor
                .loaded
                .as_ref()
                .expect("a video is open")
                .cuts
                .ranges()
                .is_empty()
        );
    }
}
