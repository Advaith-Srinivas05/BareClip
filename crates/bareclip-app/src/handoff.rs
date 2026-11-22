//! Hands videos chosen with "Edit with BareClip" in Explorer to a BareClip window that is
//! already open, so they land in the project being worked on rather than in a new window.
//!
//! Every BareClip window listens on a pipe named after its process. A launch from the
//! right-click menu looks through the windows on screen from the top down, so the one used
//! last is asked first, and gives its files to the first that listens. Explorer starts one
//! launch for each file when several are chosen. While no window is open yet, the first of
//! them takes a name only one process can hold, and the rest give their files to it.

use std::{
    collections::HashSet,
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use bareclip_core::{time::Ticks, timeline::TimeRange};
use eframe::egui;
use windows_sys::Win32::{
    Foundation::{
        ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, HWND,
        INVALID_HANDLE_VALUE, LPARAM,
    },
    Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND},
    System::{
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
        },
        RemoteDesktop::ProcessIdToSessionId,
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE,
        SetForegroundWindow, ShowWindow,
    },
};

/// The start of every pipe name the app listens on. Tests use their own, so they never
/// reach a BareClip that is already open.
pub const PIPE_PREFIX: &str = "BareClip";

/// What the right-click menu puts before the file, as the installer registers it.
pub const EDIT_FLAG: &str = "--edit";

/// What a double-click on a video or sound puts before the file, which opens the player.
pub const PLAY_FLAG: &str = "--play";

/// The start of every pipe name a player window listens on. It differs from the editors', so a
/// file opened to watch never lands in a project, and one sent to edit never lands in a player.
pub const PLAYER_PIPE_PREFIX: &str = "BareClipPlayer";

/// What the installer runs the app with after changing file types, to tell Explorer so. The app
/// does that and leaves, with no window.
pub const REFRESH_FLAG: &str = "--refresh-file-types";

/// More than any list of paths Explorer would send, so a stray writer can't fill memory.
const LARGEST_MESSAGE: u64 = 4 << 20;

/// How the app was asked to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchKind {
    /// The Start menu, a project file, or files given with no flag: the editor.
    Editor,
    /// "Edit with BareClip", so the files go on the timeline.
    Edit,
    /// A double-click on a video or sound, which opens the player.
    Play,
    /// The installer, telling Explorer that file types changed.
    RefreshFileTypes,
}

/// What the app was started with.
#[derive(Debug, PartialEq, Eq)]
pub struct Launch {
    pub kind: LaunchKind,
    pub paths: Vec<PathBuf>,
}

impl Launch {
    /// Started from "Edit with BareClip", so the files go on the timeline.
    pub fn edit(&self) -> bool {
        self.kind == LaunchKind::Edit
    }
}

/// Reads the command line. The first flag decides how the app opens, and any later flag is
/// dropped rather than taken for a file, because nothing registers more than one.
pub fn read_launch(args: impl IntoIterator<Item = OsString>) -> Launch {
    let mut kind = None;
    let mut paths = Vec::new();
    for arg in args {
        if arg == EDIT_FLAG {
            kind.get_or_insert(LaunchKind::Edit);
        } else if arg == PLAY_FLAG {
            kind.get_or_insert(LaunchKind::Play);
        } else if arg == REFRESH_FLAG {
            kind.get_or_insert(LaunchKind::RefreshFileTypes);
        } else {
            paths.push(PathBuf::from(arg));
        }
    }
    Launch {
        kind: kind.unwrap_or(LaunchKind::Editor),
        paths,
    }
}

/// What became of a launch's files.
pub enum HandOver {
    /// An open window has them, so this launch has nothing left to do.
    Delivered,
    /// No window was open, and this launch holds the shared name, so it opens the window and
    /// takes the files of launches that follow it.
    Claimed(OwnedHandle),
    /// Nothing could take them, so this launch opens a window of its own.
    Alone,
}

/// Gives the files to the BareClip window used last, or to one just starting.
pub fn hand_over(prefix: &str, paths: &[PathBuf]) -> HandOver {
    let message = encode(&paths.iter().map(|path| absolute(path)).collect::<Vec<_>>());
    let starting = starting_pipe(prefix);
    let own = std::process::id();
    // Another launch can take the shared name between looking for it and claiming it, and
    // then this one only has to look again.
    for _ in 0..50 {
        let windows: Vec<(HWND, u32)> = windows_top_down()
            .into_iter()
            .filter(|(_, process)| *process != own)
            .collect();
        if let Some(window) = deliver_to_windows(prefix, &windows, &message) {
            bring_forward(window);
            return HandOver::Delivered;
        }
        if send(&starting, &message) {
            return HandOver::Delivered;
        }
        if let Some(pipe) = create_pipe(&starting, true) {
            return HandOver::Claimed(pipe);
        }
        thread::sleep(Duration::from_millis(20));
    }
    HandOver::Alone
}

/// Tries each window in turn and says which took the files. A window that isn't BareClip
/// has no pipe, which costs one failed open.
fn deliver_to_windows(prefix: &str, windows: &[(HWND, u32)], message: &[u8]) -> Option<HWND> {
    let mut tried = HashSet::new();
    windows
        .iter()
        .filter(|(_, process)| tried.insert(*process))
        .find(|(_, process)| send(&window_pipe(prefix, *process), message))
        .map(|(window, _)| *window)
}

/// A file sent to a window. A clip sent from the player also says which stretch of it to use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handed {
    pub path: PathBuf,
    pub range: Option<TimeRange>,
}

impl Handed {
    pub fn whole(path: PathBuf) -> Self {
        Self { path, range: None }
    }
}

/// Files that arrived from other launches, waiting for the window to take them.
#[derive(Clone)]
pub struct Inbox {
    shared: Arc<Shared>,
}

struct Shared {
    arrived: Mutex<Vec<Handed>>,
    wake: OnceLock<egui::Context>,
    /// Set when the window stops listening, as when the player becomes the editor.
    stopped: AtomicBool,
    /// The names listened on, so stopping can wake each listener.
    names: Mutex<Vec<String>>,
}

impl Inbox {
    /// Starts listening for this process's window, and on the shared name too if this launch
    /// claimed it.
    pub fn listen(prefix: &str, claimed: Option<OwnedHandle>) -> Inbox {
        let inbox = Inbox::listen_as(prefix, std::process::id());
        if let Some(first) = claimed {
            inbox.serve(starting_pipe(prefix), Some(first));
        }
        inbox
    }

    /// Listens as if this process had another number, which lets tests stand in for windows.
    fn listen_as(prefix: &str, process: u32) -> Inbox {
        let inbox = Inbox {
            shared: Arc::new(Shared {
                arrived: Mutex::new(Vec::new()),
                wake: OnceLock::new(),
                stopped: AtomicBool::new(false),
                names: Mutex::new(Vec::new()),
            }),
        };
        inbox.serve(window_pipe(prefix, process), None);
        inbox
    }

    /// Lets files that arrive wake the window, which otherwise waits for input.
    pub fn wake(&self, ctx: &egui::Context) {
        let _ = self.shared.wake.set(ctx.clone());
        if !self
            .shared
            .arrived
            .lock()
            .expect("the inbox is usable")
            .is_empty()
        {
            ctx.request_repaint();
        }
    }

    /// Everything that arrived since the last call, oldest first.
    pub fn take(&self) -> Vec<Handed> {
        std::mem::take(&mut *self.shared.arrived.lock().expect("the inbox is usable"))
    }

    /// Stops listening, so nothing more arrives and the pipes' names are free.
    pub fn stop(&self) {
        self.shared.stopped.store(true, Ordering::Relaxed);
        let names = self
            .shared
            .names
            .lock()
            .expect("the inbox is usable")
            .clone();
        for name in names {
            // A listener waiting for a sender wakes, sees it has stopped, and lets go.
            let _ = send(&name, &[]);
        }
    }

    /// Waits on its own thread for one sender after another. The pipe exists before this
    /// returns, so a sender straight after finds it. The next copy is made before the last one
    /// is read, so the name never lapses for another launch to take.
    fn serve(&self, name: String, first: Option<OwnedHandle>) {
        let shared = self.shared.clone();
        shared
            .names
            .lock()
            .expect("the inbox is usable")
            .push(name.clone());
        let mut waiting = first.or_else(|| create_pipe(&name, false));
        thread::spawn(move || {
            while let Some(pipe) = waiting.take() {
                // A sender that already wrote and left still left its files to read.
                let connected = unsafe { ConnectNamedPipe(pipe_handle(&pipe), ptr::null_mut()) }
                    != 0
                    || matches!(
                        unsafe { GetLastError() },
                        ERROR_PIPE_CONNECTED | ERROR_NO_DATA
                    );
                if shared.stopped.load(Ordering::Relaxed) {
                    break;
                }
                waiting = create_pipe(&name, false);
                if !connected {
                    continue;
                }
                let mut message = Vec::new();
                // Reading to the end ends when the sender closes its side.
                if File::from(pipe)
                    .take(LARGEST_MESSAGE)
                    .read_to_end(&mut message)
                    .is_err()
                {
                    continue;
                }
                let handed = decode(&message);
                if handed.is_empty() {
                    continue;
                }
                shared
                    .arrived
                    .lock()
                    .expect("the inbox is usable")
                    .extend(handed);
                if let Some(ctx) = shared.wake.get() {
                    ctx.request_repaint();
                }
            }
        });
    }
}

fn pipe_handle(pipe: &OwnedHandle) -> HANDLE {
    use std::os::windows::io::AsRawHandle;
    pipe.as_raw_handle()
}

/// Opens a copy of a pipe to read from. `first` fails if the name is already taken, which is
/// how only one launch gets the shared name.
fn create_pipe(name: &str, first: bool) -> Option<OwnedHandle> {
    let wide = wide(name);
    let open_mode = PIPE_ACCESS_INBOUND
        | if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            0,
            64 * 1024,
            0,
            ptr::null(),
        )
    };
    (handle != INVALID_HANDLE_VALUE).then(|| unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// Writes the files to a pipe. False if nothing is listening there.
fn send(name: &str, message: &[u8]) -> bool {
    for _ in 0..20 {
        match OpenOptions::new().write(true).open(name) {
            Ok(mut pipe) => return pipe.write_all(message).is_ok(),
            // Every copy is busy with another sender, so wait for one to come free.
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => unsafe {
                WaitNamedPipeW(wide(name).as_ptr(), 1000);
            },
            Err(_) => return false,
        }
    }
    false
}

/// Pipe names are shared by everyone signed in to the PC, so the shared one carries the
/// session's number, and a window's carries its process's, which is already unique.
fn starting_pipe(prefix: &str) -> String {
    let mut session = 0u32;
    unsafe { ProcessIdToSessionId(std::process::id(), &mut session) };
    format!(r"\\.\pipe\{prefix}-starting-{session}")
}

fn window_pipe(prefix: &str, process: u32) -> String {
    format!(r"\\.\pipe\{prefix}-window-{process}")
}

/// Every window on screen and its process, the topmost first.
fn windows_top_down() -> Vec<(HWND, u32)> {
    unsafe extern "system" fn visit(window: HWND, found: LPARAM) -> windows_sys::core::BOOL {
        let found = unsafe { &mut *(found as *mut Vec<(HWND, u32)>) };
        if unsafe { IsWindowVisible(window) } != 0 {
            let mut process = 0;
            unsafe { GetWindowThreadProcessId(window, &mut process) };
            found.push((window, process));
        }
        1
    }
    let mut found: Vec<(HWND, u32)> = Vec::new();
    unsafe { EnumWindows(Some(visit), &mut found as *mut _ as LPARAM) };
    found
}

/// Brings the window that took the files to the front. Explorer started this launch, which
/// is what lets it do that.
fn bring_forward(window: HWND) {
    if window.is_null() {
        return;
    }
    unsafe {
        if IsIconic(window) != 0 {
            ShowWindow(window, SW_RESTORE);
        }
        SetForegroundWindow(window);
    }
}

/// Explorer gives full paths, but anything else could start BareClip from another folder.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_owned())
}

/// Paths as UTF-16, each ended with a zero, which keeps any name Windows allows exactly. This
/// is the first form of the message, which every version reads, so plain files are always
/// sent this way.
fn encode(paths: &[PathBuf]) -> Vec<u8> {
    paths
        .iter()
        .flat_map(|path| path.as_os_str().encode_wide().chain([0]))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// How the second form of the message starts. A version that only knows the first reads the
/// whole message as one file name, and Windows allows neither `|` nor `"` in one, so opening
/// it fails and nothing lands with the wrong stretch.
const SECOND_FORM: &str = "|BareClip message 2|";

/// One file in the second form: its name as UTF-16, so every name Windows allows survives, and
/// the stretch to use in ticks.
#[derive(serde::Serialize, serde::Deserialize)]
struct Sent {
    path: Vec<u16>,
    start: Option<i64>,
    end: Option<i64>,
}

/// Files with the stretch of each to use, in the second form, which has no zeros in it.
fn encode_clips(clips: &[Handed]) -> Vec<u8> {
    let sent: Vec<Sent> = clips
        .iter()
        .map(|clip| Sent {
            path: clip.path.as_os_str().encode_wide().collect(),
            start: clip.range.map(|range| range.start.0),
            end: clip.range.map(|range| range.end.0),
        })
        .collect();
    let body = serde_json::to_string(&sent).unwrap_or_default();
    format!("{SECOND_FORM}{body}")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// Reads either form of the message.
fn decode(message: &[u8]) -> Vec<Handed> {
    let units: Vec<u16> = message
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let mark: Vec<u16> = SECOND_FORM.encode_utf16().collect();
    if let Some(body) = units.strip_prefix(mark.as_slice()) {
        let text = String::from_utf16_lossy(body);
        let sent: Vec<Sent> = serde_json::from_str(&text).unwrap_or_default();
        return sent
            .into_iter()
            .filter(|sent| !sent.path.is_empty())
            .map(|sent| Handed {
                path: PathBuf::from(OsString::from_wide(&sent.path)),
                range: sent
                    .start
                    .zip(sent.end)
                    .filter(|(start, end)| start < end)
                    .map(|(start, end)| TimeRange {
                        start: Ticks(start),
                        end: Ticks(end),
                    }),
            })
            .collect();
    }
    decode_first_form(&units)
        .into_iter()
        .map(Handed::whole)
        .collect()
}

/// The first form: names ended with zeros. This is all a version before the second form reads.
fn decode_first_form(units: &[u16]) -> Vec<PathBuf> {
    units
        .split(|unit| *unit == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(OsString::from_wide(path)))
        .collect()
}

/// Sends a clip to the editor used last, or one just starting, bringing it to the front. Says
/// whether one took it. It never claims the name for starting, because the player turns into
/// the editor itself when none is open.
pub fn send_clip_to_editor(prefix: &str, clip: &Handed) -> bool {
    let message = encode_clips(std::slice::from_ref(clip));
    let own = std::process::id();
    let windows: Vec<(HWND, u32)> = windows_top_down()
        .into_iter()
        .filter(|(_, process)| *process != own)
        .collect();
    if let Some(window) = deliver_to_windows(prefix, &windows, &message) {
        bring_forward(window);
        return true;
    }
    send(&starting_pipe(prefix), &message)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// An editor just starting, under a name for tests, which takes what is sent to it.
#[cfg(test)]
pub fn starting_editor(prefix: &str) -> Inbox {
    let inbox = Inbox::listen_as(prefix, 1);
    inbox.serve(starting_pipe(prefix), None);
    inbox
}

/// Whether this process's window answers under a name, for tests.
#[cfg(test)]
pub fn window_answers(prefix: &str) -> bool {
    send(&window_pipe(prefix, std::process::id()), &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU32, Ordering},
        time::Instant,
    };

    /// A name no other test, and no real BareClip, is using.
    fn test_prefix() -> String {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        format!(
            "BareClipTest-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn wait_for(inbox: &Inbox, count: usize) -> Vec<PathBuf> {
        let started = Instant::now();
        let mut arrived = Vec::new();
        while arrived.len() < count {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "only {} of {count} arrived",
                arrived.len()
            );
            arrived.extend(inbox.take().into_iter().map(|handed| handed.path));
            thread::sleep(Duration::from_millis(5));
        }
        arrived
    }

    #[test]
    fn edit_is_told_apart_from_the_files() {
        let launch = read_launch(["--edit", r"C:\a b.mp4"].map(OsString::from));
        assert_eq!(
            launch,
            Launch {
                kind: LaunchKind::Edit,
                paths: vec![PathBuf::from(r"C:\a b.mp4")]
            }
        );
        assert!(launch.edit());

        let launch = read_launch([r"C:\project.bareclip"].map(OsString::from));
        assert_eq!(launch.kind, LaunchKind::Editor);
        assert!(!launch.edit());
        assert_eq!(launch.paths, [PathBuf::from(r"C:\project.bareclip")]);
    }

    #[test]
    fn play_opens_the_player_with_every_file_kept_in_order() {
        let launch = read_launch(["--play", r"C:\a.mp4", r"C:\b.mp3"].map(OsString::from));
        assert_eq!(
            launch,
            Launch {
                kind: LaunchKind::Play,
                paths: vec![PathBuf::from(r"C:\a.mp4"), PathBuf::from(r"C:\b.mp3")]
            }
        );
        assert!(!launch.edit());

        let launch = read_launch([r"C:\a.mp4", "--play"].map(OsString::from));
        assert_eq!(launch.kind, LaunchKind::Play, "the flag can come after");
        assert_eq!(launch.paths, [PathBuf::from(r"C:\a.mp4")]);

        let launch = read_launch(std::iter::empty());
        assert_eq!(launch.kind, LaunchKind::Editor);
        assert!(launch.paths.is_empty());
    }

    #[test]
    fn the_installers_refresh_is_told_apart_and_takes_no_files() {
        let launch = read_launch(["--refresh-file-types"].map(OsString::from));
        assert_eq!(launch.kind, LaunchKind::RefreshFileTypes);
        assert!(launch.paths.is_empty());
        assert!(!launch.edit());
    }
    #[test]
    fn the_first_flag_decides_and_later_flags_are_not_taken_for_files() {
        let launch = read_launch(["--play", r"C:\a.mp4", "--edit"].map(OsString::from));
        assert_eq!(launch.kind, LaunchKind::Play);
        assert_eq!(launch.paths, [PathBuf::from(r"C:\a.mp4")]);

        let launch = read_launch(["--edit", "--play", r"C:\a.mp4"].map(OsString::from));
        assert_eq!(launch.kind, LaunchKind::Edit);
        assert_eq!(launch.paths, [PathBuf::from(r"C:\a.mp4")]);
    }

    #[test]
    fn a_file_to_play_reaches_a_player_and_never_an_editor() {
        let editors = format!("{}-edit", test_prefix());
        let players = format!("{}-play", test_prefix());
        let editor = Inbox::listen_as(&editors, 610);
        let player = Inbox::listen_as(&players, 620);
        let windows = [(ptr::null_mut(), 610), (ptr::null_mut(), 620)];
        let message = encode(&[PathBuf::from("watch.mp4")]);

        assert!(deliver_to_windows(&players, &windows, &message).is_some());
        assert_eq!(wait_for(&player, 1), [PathBuf::from("watch.mp4")]);
        thread::sleep(Duration::from_millis(50));
        assert!(editor.take().is_empty(), "the editor on top is passed over");

        let message = encode(&[PathBuf::from("edit.mp4")]);
        assert!(deliver_to_windows(&editors, &windows, &message).is_some());
        assert_eq!(wait_for(&editor, 1), [PathBuf::from("edit.mp4")]);
        thread::sleep(Duration::from_millis(50));
        assert!(
            player.take().is_empty(),
            "a player never gets files to edit"
        );
    }

    #[test]
    fn players_and_editors_gather_launches_on_names_of_their_own() {
        let editors = format!("{}-edit", test_prefix());
        let players = format!("{}-play", test_prefix());
        assert_ne!(starting_pipe(&editors), starting_pipe(&players));
        let HandOver::Claimed(_editor) = hand_over(&editors, &[PathBuf::from("a.mp4")]) else {
            panic!("nothing is open, so the first editor launch claims the name");
        };
        let HandOver::Claimed(_player) = hand_over(&players, &[PathBuf::from("b.mp4")]) else {
            panic!("a player launch claims its own name, not the editors'");
        };
    }

    #[test]
    fn any_name_windows_allows_survives_the_trip() {
        // An unpaired surrogate can't be written as UTF-8, but Windows allows it in a name.
        let odd = OsString::from_wide(&[0x0061, 0xD800, 0x0062]);
        let paths = vec![
            PathBuf::from(r"C:\Videos\clip one.mp4"),
            PathBuf::from(r"D:\वीडियो\動画 ✂.mkv"),
            PathBuf::from(odd),
        ];

        let back: Vec<PathBuf> = decode(&encode(&paths))
            .into_iter()
            .map(|handed| handed.path)
            .collect();
        assert_eq!(back, paths);
        assert!(decode(&[]).is_empty());
    }

    #[test]
    fn a_listening_window_receives_the_files_sent_to_it() {
        let prefix = test_prefix();
        let inbox = Inbox::listen_as(&prefix, 4242);
        let paths = vec![PathBuf::from(r"C:\a.mp4"), PathBuf::from(r"C:\b.mp3")];

        assert!(send(&window_pipe(&prefix, 4242), &encode(&paths)));

        assert_eq!(wait_for(&inbox, 2), paths);
        assert!(inbox.take().is_empty(), "taking empties it");
    }

    #[test]
    fn nothing_listening_means_nothing_is_delivered() {
        let prefix = test_prefix();
        assert!(!send(
            &window_pipe(&prefix, 1),
            &encode(&[PathBuf::from("a.mp4")])
        ));
    }

    #[test]
    fn the_first_window_that_listens_gets_them_and_windows_without_a_pipe_are_skipped() {
        let prefix = test_prefix();
        let lower = Inbox::listen_as(&prefix, 300);
        let upper = Inbox::listen_as(&prefix, 200);
        let windows = [
            (ptr::null_mut(), 100),
            (ptr::null_mut(), 200),
            (ptr::null_mut(), 300),
        ];

        let delivered = deliver_to_windows(&prefix, &windows, &encode(&[PathBuf::from("a.mp4")]));

        assert!(delivered.is_some());
        assert_eq!(wait_for(&upper, 1), [PathBuf::from("a.mp4")]);
        thread::sleep(Duration::from_millis(50));
        assert!(
            lower.take().is_empty(),
            "only the topmost BareClip gets them"
        );
    }

    #[test]
    fn only_one_launch_can_hold_the_shared_name_and_the_rest_reach_it() {
        let prefix = test_prefix();
        let name = starting_pipe(&prefix);
        let first = create_pipe(&name, true).expect("the first launch gets the name");
        assert!(create_pipe(&name, true).is_none(), "a second launch can't");

        let inbox = Inbox::listen_as(&prefix, 9001);
        inbox.serve(name.clone(), Some(first));
        assert!(send(&name, &encode(&[PathBuf::from("second.mp4")])));

        assert_eq!(wait_for(&inbox, 1), [PathBuf::from("second.mp4")]);
        assert!(
            create_pipe(&name, true).is_none(),
            "the name stays taken while it listens"
        );
    }

    #[test]
    fn many_launches_at_once_all_get_through() {
        let prefix = test_prefix();
        let inbox = Inbox::listen_as(&prefix, 77);
        let name = window_pipe(&prefix, 77);

        let senders: Vec<_> = (0..12)
            .map(|index| {
                let name = name.clone();
                thread::spawn(move || {
                    send(&name, &encode(&[PathBuf::from(format!("{index}.mp4"))]))
                })
            })
            .collect();
        for sender in senders {
            assert!(sender.join().unwrap(), "every sender got through");
        }

        let mut arrived = wait_for(&inbox, 12);
        arrived.sort();
        let mut expected: Vec<PathBuf> = (0..12)
            .map(|index| PathBuf::from(format!("{index}.mp4")))
            .collect();
        expected.sort();
        assert_eq!(arrived, expected);
    }

    #[test]
    fn launches_with_no_window_open_gather_in_the_first() {
        let prefix = test_prefix();
        let HandOver::Claimed(first) = hand_over(&prefix, &[PathBuf::from(r"C:\one.mp4")]) else {
            panic!("with nothing open, the first launch opens the window");
        };
        let inbox = Inbox::listen_as(&prefix, 5150);
        inbox.serve(starting_pipe(&prefix), Some(first));

        for name in ["two.mp4", "three.mp3"] {
            let path = PathBuf::from(format!(r"C:\{name}"));
            assert!(matches!(hand_over(&prefix, &[path]), HandOver::Delivered));
        }

        assert_eq!(
            wait_for(&inbox, 2),
            [PathBuf::from(r"C:\two.mp4"), PathBuf::from(r"C:\three.mp3")]
        );
    }

    fn seconds(start: f64, end: f64) -> TimeRange {
        TimeRange {
            start: Ticks::from_seconds(start),
            end: Ticks::from_seconds(end),
        }
    }

    #[test]
    fn clips_with_and_without_a_stretch_survive_the_trip() {
        let odd = OsString::from_wide(&[0x0061, 0xD800, 0x0062]);
        let clips = vec![
            Handed {
                path: PathBuf::from(r"D:\वीडियो\動画 ✂.mkv"),
                range: Some(seconds(1.5, 4.25)),
            },
            Handed::whole(PathBuf::from(odd)),
        ];
        assert_eq!(decode(&encode_clips(&clips)), clips);
    }

    #[test]
    fn a_message_from_an_older_version_reads_as_whole_files() {
        let paths = vec![PathBuf::from(r"C:\a.mp4"), PathBuf::from(r"C:\b.mp3")];
        assert_eq!(
            decode(&encode(&paths)),
            paths.into_iter().map(Handed::whole).collect::<Vec<_>>()
        );
    }

    /// An editor from before the second form, still open after an upgrade, must not add a clip
    /// with the wrong stretch. It reads the message as one name that can't be a file.
    #[test]
    fn an_older_version_given_a_clip_finds_nothing_it_can_open() {
        let clip = Handed {
            path: PathBuf::from(r"C:\clip.mp4"),
            range: Some(seconds(2.0, 3.0)),
        };
        let units: Vec<u16> = encode_clips(std::slice::from_ref(&clip))
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let read_by_older = decode_first_form(&units);
        assert_eq!(read_by_older.len(), 1, "one name, not the clip's path");
        let name = read_by_older[0].to_string_lossy().into_owned();
        assert!(name.contains('|') && name.contains('"'), "{name}");
        assert_ne!(read_by_older[0], clip.path);
        assert!(std::fs::File::open(&read_by_older[0]).is_err());
    }

    #[test]
    fn a_clip_reaches_a_listening_editor_with_its_stretch() {
        let prefix = test_prefix();
        let editor = Inbox::listen_as(&prefix, 7300);
        let windows = [(ptr::null_mut(), 7300)];
        let clip = Handed {
            path: PathBuf::from(r"C:\clip.mp4"),
            range: Some(seconds(0.5, 2.0)),
        };
        let message = encode_clips(std::slice::from_ref(&clip));
        assert!(deliver_to_windows(&prefix, &windows, &message).is_some());
        let started = Instant::now();
        let mut arrived = Vec::new();
        while arrived.is_empty() && started.elapsed() < Duration::from_secs(10) {
            arrived = editor.take();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(arrived, [clip]);

        // With no editor listening, nothing takes it.
        assert!(!send_clip_to_editor(
            &test_prefix(),
            &Handed::whole(PathBuf::from("a.mp4"))
        ));
    }

    #[test]
    fn a_window_that_stops_listening_gets_nothing_more() {
        let prefix = test_prefix();
        let inbox = Inbox::listen_as(&prefix, 8800);
        let name = window_pipe(&prefix, 8800);
        assert!(send(&name, &encode(&[PathBuf::from("first.mp4")])));
        assert_eq!(wait_for(&inbox, 1), [PathBuf::from("first.mp4")]);

        inbox.stop();
        let started = Instant::now();
        while send(&name, &encode(&[PathBuf::from("later.mp4")]))
            && started.elapsed() < Duration::from_secs(5)
        {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !send(&name, &encode(&[PathBuf::from("later.mp4")])),
            "the pipe is gone"
        );
        thread::sleep(Duration::from_millis(50));
        assert!(
            inbox
                .take()
                .iter()
                .all(|handed| handed.path != Path::new("later.mp4"))
        );
    }
    #[test]
    fn a_relative_path_is_sent_in_full() {
        let full = absolute(Path::new("clip.mp4"));
        assert!(full.is_absolute());
        assert!(full.ends_with("clip.mp4"));
    }

    #[test]
    fn the_windows_on_screen_can_be_listed() {
        // There may be no windows on a build machine, so this only checks it doesn't fail and
        // never lists a hidden one twice by mistake.
        let windows = windows_top_down();
        let unique: HashSet<HWND> = windows.iter().map(|(window, _)| *window).collect();
        assert_eq!(unique.len(), windows.len());
    }
}
