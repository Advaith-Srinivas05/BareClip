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
    sync::{Arc, Mutex, OnceLock},
    thread,
    time::Duration,
};

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

/// More than any list of paths Explorer would send, so a stray writer can't fill memory.
const LARGEST_MESSAGE: u64 = 4 << 20;

/// What the app was started with.
#[derive(Debug, PartialEq, Eq)]
pub struct Launch {
    /// Started from "Edit with BareClip", so the files go on the timeline.
    pub edit: bool,
    pub paths: Vec<PathBuf>,
}

pub fn read_launch(args: impl IntoIterator<Item = OsString>) -> Launch {
    let mut edit = false;
    let mut paths = Vec::new();
    for arg in args {
        if arg == EDIT_FLAG {
            edit = true;
        } else {
            paths.push(PathBuf::from(arg));
        }
    }
    Launch { edit, paths }
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

/// Files that arrived from other launches, waiting for the editor to take them.
#[derive(Clone)]
pub struct Inbox {
    shared: Arc<Shared>,
}

struct Shared {
    arrived: Mutex<Vec<PathBuf>>,
    wake: OnceLock<egui::Context>,
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
    pub fn take(&self) -> Vec<PathBuf> {
        std::mem::take(&mut *self.shared.arrived.lock().expect("the inbox is usable"))
    }

    /// Waits on its own thread for one sender after another. The pipe exists before this
    /// returns, so a sender straight after finds it. The next copy is made before the last one
    /// is read, so the name never lapses for another launch to take.
    fn serve(&self, name: String, first: Option<OwnedHandle>) {
        let shared = self.shared.clone();
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
                let paths = decode(&message);
                if paths.is_empty() {
                    continue;
                }
                shared
                    .arrived
                    .lock()
                    .expect("the inbox is usable")
                    .extend(paths);
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

/// Paths as UTF-16, each ended with a zero, which keeps any name Windows allows exactly.
fn encode(paths: &[PathBuf]) -> Vec<u8> {
    paths
        .iter()
        .flat_map(|path| path.as_os_str().encode_wide().chain([0]))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn decode(message: &[u8]) -> Vec<PathBuf> {
    let units: Vec<u16> = message
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    units
        .split(|unit| *unit == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(OsString::from_wide(path)))
        .collect()
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
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
            arrived.extend(inbox.take());
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
                edit: true,
                paths: vec![PathBuf::from(r"C:\a b.mp4")]
            }
        );

        let launch = read_launch([r"C:\project.bareclip"].map(OsString::from));
        assert!(!launch.edit);
        assert_eq!(launch.paths, [PathBuf::from(r"C:\project.bareclip")]);
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

        assert_eq!(decode(&encode(&paths)), paths);
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
