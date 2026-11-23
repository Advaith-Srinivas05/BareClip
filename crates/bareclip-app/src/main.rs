#![cfg_attr(not(test), windows_subsystem = "windows")]

mod components;
mod editor;
mod file_ops;
mod folder_files;
mod fonts;
mod handoff;
mod hotkeys;
mod icons;
mod layers;
mod player;
mod preview;
#[cfg(test)]
mod screen_camera;
mod stage;
mod storage;
mod text;
mod theme;
mod timeline;
mod title_frames;
mod viewer;
mod window_place;

use std::{cell::RefCell, rc::Rc, sync::Arc};

use bareclip_render::GpuId;
use eframe::egui;

const WINDOW_ICON: &[u8] = include_bytes!("../../../assets/brand/png/app-icon-64.png");
/// Picks a chip the way Settings does, such as `1002:1638`, for measuring without touching the
/// real settings file.
const GPU_VARIABLE: &str = "BARECLIP_GPU";

fn main() -> eframe::Result {
    let launch = handoff::read_launch(std::env::args_os().skip(1));
    if launch.kind == handoff::LaunchKind::RefreshFileTypes {
        tell_explorer_file_types_changed();
        return Ok(());
    }
    let prefix = match launch.kind {
        handoff::LaunchKind::Play => handoff::PLAYER_PIPE_PREFIX,
        _ => handoff::PIPE_PREFIX,
    };
    // "Edit with BareClip" goes to an editor already open, and a file to watch to a player
    // already open, if there is one.
    let mut claimed = None;
    if launch.kind != handoff::LaunchKind::Editor && !launch.new_window {
        // A player is only handed the first file. The rest are what next and previous find.
        let handed = match launch.kind {
            handoff::LaunchKind::Play => &launch.paths[..launch.paths.len().min(1)],
            _ => &launch.paths[..],
        };
        match handoff::hand_over(prefix, handed) {
            handoff::HandOver::Delivered => return Ok(()),
            handoff::HandOver::Claimed(name) => claimed = Some(name),
            handoff::HandOver::Alone => {}
        }
    }
    let inbox = handoff::Inbox::listen(prefix, claimed);

    let wanted = wanted_gpu();
    bareclip_render::choose_gpu(wanted);
    let waiting = Rc::new(RefCell::new(Some((launch, inbox))));
    let result = open_window(wanted, waiting.clone());
    // A chip that can't open the window must not stop the app starting, because Settings is the
    // only way to change it. Once the window has been up, it isn't the chip's fault it closed.
    if wanted.is_some() && result.is_err() && waiting.borrow().is_some() {
        bareclip_render::choose_gpu(None);
        return open_window(None, waiting);
    }
    result
}

/// Tells Explorer that which programs open which files changed, so icons and Open with show it
/// straight away.
fn tell_explorer_file_types_changed() {
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    // SAFETY: this notice takes no items, so both are null.
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
}

/// What opens the app with, which the first window to open takes.
type WaitingLaunch = Rc<RefCell<Option<(handoff::Launch, handoff::Inbox)>>>;

/// The chip picked in Settings, or the one the hidden switch names for a test.
fn wanted_gpu() -> Option<GpuId> {
    std::env::var(GPU_VARIABLE)
        .ok()
        .and_then(|text| GpuId::parse(&text))
        .or_else(|| {
            storage::app_folder()
                .and_then(|folder| storage::Settings::load(&folder).gpu)
                .map(GpuId::from)
        })
}

/// The picked chip if it can show the window, and otherwise the one Automatic would use.
fn pick_window_chip(
    adapters: &[eframe::wgpu::Adapter],
    surface: Option<&eframe::wgpu::Surface<'_>>,
    wanted: GpuId,
) -> Result<eframe::wgpu::Adapter, String> {
    let able: Vec<&eframe::wgpu::Adapter> = adapters
        .iter()
        .filter(|adapter| surface.is_none_or(|surface| adapter.is_surface_supported(surface)))
        .collect();
    let infos: Vec<eframe::wgpu::AdapterInfo> =
        able.iter().map(|adapter| adapter.get_info()).collect();
    bareclip_render::find_gpu(&infos, wanted)
        .or_else(|| bareclip_render::automatic_gpu(&infos))
        .map(|index| able[index].clone())
        .ok_or_else(|| "No graphics chip here can show the window.".to_owned())
}

fn open_window(wanted: Option<GpuId>, waiting: WaitingLaunch) -> eframe::Result {
    let playing = waiting
        .borrow()
        .as_ref()
        .is_some_and(|(launch, _)| launch.kind == handoff::LaunchKind::Play);
    // The player opens at 1280x720 the first time, and goes down to mini view's size.
    let (size, smallest) = if playing {
        ([1280.0, 720.0], viewer::SMALLEST_WINDOW)
    } else {
        ([1280.0, 800.0], editor::SMALLEST_WINDOW)
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("BareClip")
        .with_inner_size(size)
        .with_min_inner_size(smallest)
        .with_decorations(false)
        .with_drag_and_drop(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(WINDOW_ICON) {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport,
        wgpu_options: window_gpu_options(wanted),
        ..Default::default()
    };

    eframe::run_native(
        "BareClip",
        options,
        Box::new(move |creation_context| {
            let (launch, inbox) = waiting
                .borrow_mut()
                .take()
                .ok_or("The app can only start once.")?;
            let ctx = &creation_context.egui_ctx;
            fonts::install(ctx);
            theme::install(ctx);
            let gpu = creation_context.wgpu_render_state.clone();
            if let Some(gpu) = &gpu
                && wanted.is_some()
            {
                // Only the chip the window really got decides the encoders, so one that has
                // gone missing behaves like Automatic.
                bareclip_media::follow_chip(bareclip_media::ChipMaker::from_vendor(
                    gpu.adapter.get_info().vendor,
                ));
            }
            // The window is still hidden here, so it never shows anywhere else first.
            let window = window_handle(creation_context);
            if let Some(window) = window {
                give_window_the_apps_icon(window);
            }
            let (face, maximize) = match launch.kind {
                handoff::LaunchKind::Play => {
                    let mut viewer = viewer::Viewer::new(ctx, true);
                    viewer.use_window(window);
                    if let Some(folder) = storage::app_folder() {
                        viewer.use_storage(ctx, folder);
                    }
                    let maximize = window
                        .is_some_and(|window| window_place::restore(window, viewer.saved_window()));
                    viewer.open_launch(ctx, &launch.paths);
                    (Face::Viewer(Box::new(viewer)), maximize)
                }
                _ => {
                    let mut editor = build_editor(ctx, gpu.as_ref(), storage::app_folder(), true);
                    let maximize = window
                        .is_some_and(|window| window_place::restore(window, editor.saved_window()));
                    if launch.edit() {
                        editor.edit_files(&launch.paths);
                    } else if !launch.paths.is_empty() {
                        editor.import(ctx, &launch.paths);
                    }
                    (Face::Editor(Box::new(editor)), maximize)
                }
            };
            inbox.wake(ctx);
            Ok(Box::new(Shell {
                face,
                inbox,
                window,
                maximize,
                gpu,
                app_folder: storage::app_folder(),
                editor_prefix: handoff::PIPE_PREFIX.to_owned(),
                use_sound: true,
                open_elsewhere: open_in_new_player,
            }))
        }),
    )
}

/// An editor set up for this window: its GPU, its settings and recovery, and the chips there
/// are to pick from.
fn build_editor(
    ctx: &egui::Context,
    gpu: Option<&eframe::egui_wgpu::RenderState>,
    folder: Option<std::path::PathBuf>,
    use_sound: bool,
) -> editor::Editor {
    let mut editor = editor::Editor::new(ctx, use_sound);
    if let Some(gpu) = gpu {
        editor.use_gpu(layers::LayerDisplay::on_window(gpu));
    }
    if let Some(folder) = folder {
        editor.use_storage(ctx, folder);
    }
    if let Some(gpu) = gpu {
        let chips = gpu
            .available_adapters
            .iter()
            .map(eframe::wgpu::Adapter::get_info)
            .filter(bareclip_render::is_real_chip)
            .map(|info| editor::GpuEntry::of(&info))
            .collect();
        editor.list_gpus(editor::GpuEntry::of(&gpu.adapter.get_info()), chips);
    }
    editor
}

/// The window only asks for DirectX 12. Asking for every graphics system on the machine loads
/// drivers for all of them, including old OpenGL ones that have deadlocked here while loading,
/// and the compositor shares this GPU for showing layers.
fn window_gpu_options(wanted: Option<GpuId>) -> eframe::egui_wgpu::WgpuConfiguration {
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
    if let Some(wanted) = wanted {
        setup.native_adapter_selector = Some(Arc::new(move |adapters, surface| {
            pick_window_chip(adapters, surface, wanted)
        }));
    }
    eframe::egui_wgpu::WgpuConfiguration {
        wgpu_setup: setup.into(),
        ..Default::default()
    }
}

/// The window as Windows knows it.
fn window_handle(
    window: &impl eframe::wgpu::rwh::HasWindowHandle,
) -> Option<windows_sys::Win32::Foundation::HWND> {
    match window.window_handle().ok()?.as_raw() {
        eframe::wgpu::rwh::RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as _),
        _ => None,
    }
}

/// The app's icon in the exe, as the build puts it there.
const APP_ICON_RESOURCE: u16 = 1;

/// Gives the window the app's icon from the exe, big for the taskbar and Alt+Tab and small for
/// the title. egui only gives it a small one made from a picture, and without a big one the
/// taskbar shows Windows' plain application icon.
fn give_window_the_apps_icon(window: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, ICON_BIG, ICON_SMALL, SM_CXICON, SM_CXSMICON, SendMessageW, WM_SETICON,
    };
    for (kind, metric) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)] {
        // SAFETY: asking a size has no inputs that can be wrong.
        let side = unsafe { GetSystemMetrics(metric) };
        if let Some(icon) = app_icon(side) {
            // SAFETY: the window is this app's own, and an icon loaded from the exe stays valid
            // for as long as the app runs.
            unsafe { SendMessageW(window, WM_SETICON, kind as usize, icon as isize) };
        }
    }
}

/// The app's icon from its own exe, at a side in pixels, picking the nearest size it holds.
fn app_icon(side: i32) -> Option<windows_sys::Win32::UI::WindowsAndMessaging::HICON> {
    use windows_sys::Win32::{
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW},
    };
    // SAFETY: a null name asks for this process's own exe, and a resource number in place of
    // a name is how Windows takes one.
    let icon = unsafe {
        LoadImageW(
            GetModuleHandleW(std::ptr::null()),
            APP_ICON_RESOURCE as usize as *const u16,
            IMAGE_ICON,
            side,
            side,
            LR_DEFAULTCOLOR,
        )
    };
    (!icon.is_null()).then_some(icon)
}

/// What the window is showing: the player, or the editor.
enum Face {
    Viewer(Box<viewer::Viewer>),
    Editor(Box<editor::Editor>),
    /// Nothing, for the moment between the player going and the editor coming.
    Empty,
}

/// The one window, which is a player or an editor.
struct Shell {
    face: Face,
    /// Files from launches after this window opened: to edit for an editor, or to watch for a
    /// player.
    inbox: handoff::Inbox,
    window: Option<windows_sys::Win32::Foundation::HWND>,
    /// Whether the window closed maximized last time and has yet to be maximized again.
    maximize: bool,
    /// The window's GPU, kept so the player can become an editor in the same window.
    gpu: Option<eframe::egui_wgpu::RenderState>,
    /// Where settings live, and the name editors listen under. Tests use their own.
    app_folder: Option<std::path::PathBuf>,
    editor_prefix: String,
    /// Whether the editor it becomes plays sound. Tests have none.
    use_sound: bool,
    /// Opens a file in a player window of its own. Tests swap it, so nothing starts.
    open_elsewhere: fn(&std::path::Path),
}

impl Shell {
    /// Turns the player into the editor, in the same window, with a new project holding the
    /// clip. The player goes first, so its decoders and sound are gone before the editor opens
    /// the file again, and the window stops listening as a player and listens as an editor.
    fn become_editor(&mut self, ctx: &egui::Context, clip: handoff::Handed) {
        let place = self.window.and_then(window_place::read);
        if let Face::Viewer(viewer) = &mut self.face {
            viewer.closing(place);
        }
        self.face = Face::Empty;
        self.inbox.stop();
        self.inbox = handoff::Inbox::listen(&self.editor_prefix, None);
        self.inbox.wake(ctx);

        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::Normal,
        ));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(
            editor::SMALLEST_WINDOW.into(),
        ));
        let mut editor = build_editor(
            ctx,
            self.gpu.as_ref(),
            self.app_folder.clone(),
            self.use_sound,
        );
        match (self.window, editor.saved_window()) {
            (Some(window), Some(saved)) if window_place::put_back(window, saved) => {}
            _ => ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1280.0, 800.0))),
        }
        editor.edit_clips(&[clip]);
        self.face = Face::Editor(Box::new(editor));
    }
}

/// What a player does with files sent to it: it shows the newest, because one file shows at a
/// time. While trimming it keeps to its file, so the newest opens in a window of its own.
fn take_handed(
    viewer: &mut viewer::Viewer,
    ctx: &egui::Context,
    arrived: &[handoff::Handed],
    open_elsewhere: fn(&std::path::Path),
) {
    let Some(handed) = arrived.last() else {
        return;
    };
    if viewer.is_trimming() {
        open_elsewhere(&handed.path);
    } else {
        viewer.open(ctx, &handed.path);
    }
}

/// Starts another player window on a file, which opens on its own rather than handing the file
/// back to this one.
fn open_in_new_player(path: &std::path::Path) {
    if let Ok(app) = std::env::current_exe() {
        let _ = std::process::Command::new(app)
            .args([handoff::PLAY_FLAG, handoff::NEW_WINDOW_FLAG])
            .arg(path)
            .spawn();
    }
}

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if std::mem::take(&mut self.maximize) {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        let arrived = self.inbox.take();
        let mut becoming = None;
        match &mut self.face {
            Face::Editor(editor) => {
                if !arrived.is_empty() {
                    editor.edit_clips(&arrived);
                }
                editor.show(ui);
            }
            Face::Viewer(viewer) => {
                take_handed(viewer, ui.ctx(), &arrived, self.open_elsewhere);
                viewer.show(ui);
                becoming = viewer.take_become_editor();
            }
            Face::Empty => {}
        }
        if let Some(clip) = becoming {
            self.become_editor(ui.ctx(), clip);
            ui.ctx().request_repaint();
        }
    }

    fn on_exit(&mut self) {
        let place = self.window.and_then(window_place::read);
        match &mut self.face {
            Face::Editor(editor) => {
                if let Some(place) = place {
                    editor.remember_window(place);
                }
            }
            Face::Viewer(viewer) => viewer.closing(place),
            Face::Empty => {}
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_icon_is_a_valid_image() {
        let icon = eframe::icon_data::from_png_bytes(WINDOW_ICON).unwrap();

        assert_eq!((icon.width, icon.height), (64, 64));
    }

    /// The taskbar's icon comes from the exe, so the exe has to hold it at the sizes asked for.
    #[test]
    fn the_exe_holds_the_apps_icon_for_the_taskbar() {
        use windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon;
        for side in [16, 32, 48, 256] {
            let icon = app_icon(side).unwrap_or_else(|| panic!("a {side} px icon"));
            unsafe { DestroyIcon(icon) };
        }
    }

    /// Files a test's player sent to a window of their own.
    static SENT_ELSEWHERE: std::sync::Mutex<Vec<std::path::PathBuf>> =
        std::sync::Mutex::new(Vec::new());

    #[test]
    fn a_file_sent_while_trimming_opens_in_a_window_of_its_own() {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        theme::install(&ctx);
        let folder =
            std::env::temp_dir().join(format!("bareclip-shell-trim-tests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let [first, second] = ["first.mp4", "second.mp4"].map(|name| {
            let video = folder.join(name);
            let status = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(["-f", "lavfi", "-i", "testsrc2=s=160x90:r=30:d=2"])
                .args(["-c:v", "libopenh264"])
                .arg(&video)
                .status()
                .unwrap();
            assert!(status.success());
            video
        });
        let sent_elsewhere: fn(&std::path::Path) =
            |path| SENT_ELSEWHERE.lock().unwrap().push(path.to_owned());
        let handed = |path: &std::path::Path| handoff::Handed::whole(path.to_owned());
        let mut viewer = viewer::Viewer::new(&ctx, false);
        viewer.open(&ctx, &first);
        let first = viewer.current_path().unwrap().to_owned();

        viewer.open_trimmer(&ctx);
        take_handed(&mut viewer, &ctx, &[handed(&second)], sent_elsewhere);
        assert!(viewer.is_trimming(), "the trim stays");
        assert_eq!(viewer.current_path(), Some(first.as_path()));
        assert_eq!(
            *SENT_ELSEWHERE.lock().unwrap(),
            std::slice::from_ref(&second)
        );

        viewer.close_trimmer();
        take_handed(&mut viewer, &ctx, &[handed(&second)], sent_elsewhere);
        assert_eq!(
            viewer.current_path().and_then(|path| path.file_name()),
            second.file_name(),
            "not trimming, the window shows it"
        );
        assert_eq!(SENT_ELSEWHERE.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_player_becomes_the_editor_in_the_same_window_holding_the_clip() {
        use bareclip_core::{time::Ticks, timeline::TimeRange};
        use std::time::{Duration, Instant};

        let ctx = egui::Context::default();
        fonts::install(&ctx);
        theme::install(&ctx);
        let id = std::process::id();
        let folder = std::env::temp_dir().join(format!("bareclip-shell-tests-{id}"));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let video = folder.join("clip.mp4");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .args(["testsrc2=s=160x90:r=30:d=3", "-c:v", "libopenh264"])
            .arg(&video)
            .status()
            .unwrap();
        assert!(status.success());

        let (players, editors) = (format!("ShellTestPlay{id}"), format!("ShellTestEdit{id}"));
        let mut shell = Shell {
            face: Face::Viewer(Box::new(viewer::Viewer::new(&ctx, false))),
            inbox: handoff::Inbox::listen(&players, None),
            window: None,
            maximize: false,
            gpu: None,
            app_folder: Some(folder.clone()),
            editor_prefix: editors.clone(),
            use_sound: false,
            open_elsewhere: |_| {},
        };
        assert!(handoff::window_answers(&players));
        assert!(!handoff::window_answers(&editors));

        let range = TimeRange {
            start: Ticks::from_seconds(0.5),
            end: Ticks::from_seconds(2.0),
        };
        shell.become_editor(
            &ctx,
            handoff::Handed {
                path: video,
                range: Some(range),
            },
        );
        let Face::Editor(editor) = &mut shell.face else {
            panic!("it is the editor now");
        };
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| editor.show(ui));
            output.textures_delta.clear();
        }
        assert_eq!(
            editor.clip_sources(),
            [range],
            "a new project holding the clip"
        );

        assert!(handoff::window_answers(&editors), "it listens as an editor");
        let started = Instant::now();
        while handoff::window_answers(&players) {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "it stops listening as a player"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
