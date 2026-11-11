//! Keys that work while another program has focus, such as a game being recorded, and keeping
//! BareClip's own windows out of what is recorded.

use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        Input::KeyboardAndMouse::{
            MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
        },
        WindowsAndMessaging::{
            EnumThreadWindows, GetMessageW, MSG, PostThreadMessageW, SetWindowDisplayAffinity,
            WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WM_HOTKEY, WM_QUIT,
        },
    },
};

/// What a key asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingKey {
    Stop,
    Pause,
}

/// Ctrl+Alt+Shift with a letter, which no game or program is likely to use.
pub const STOP_KEY: (u32, &str) = (b'R' as u32, "Ctrl+Alt+Shift+R");
pub const PAUSE_KEY: (u32, &str) = (b'P' as u32, "Ctrl+Alt+Shift+P");

/// The keys, held for as long as this lives. Each press is sent down the channel, and `wake` is
/// called so the interface looks at it straight away.
pub struct RecordingKeys {
    thread_id: u32,
    worker: Option<JoinHandle<()>>,
    /// Whether Windows let both keys be taken. Another program may already hold one.
    pub taken: bool,
}

/// Tests run side by side in one program, where only one can hold the keys at a time, so each
/// waits its turn rather than finding them taken.
#[cfg(test)]
mod turns {
    use std::sync::{Condvar, Mutex};

    static HELD: Mutex<bool> = Mutex::new(false);
    static FREED: Condvar = Condvar::new();

    pub fn wait() {
        let mut held = HELD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *held {
            held = FREED
                .wait(held)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *held = true;
    }

    pub fn done() {
        *HELD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        FREED.notify_one();
    }
}

impl RecordingKeys {
    pub fn listen(sender: mpsc::Sender<RecordingKey>, wake: impl Fn() + Send + 'static) -> Self {
        #[cfg(test)]
        turns::wait();
        let (started, ready) = mpsc::channel();
        let worker = thread::spawn(move || {
            let modifiers = MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT;
            // SAFETY: the keys belong to this thread, which takes its own messages until told to
            // stop, then lets the keys go.
            unsafe {
                let stop = RegisterHotKey(std::ptr::null_mut(), 1, modifiers, STOP_KEY.0) != 0;
                let pause = RegisterHotKey(std::ptr::null_mut(), 2, modifiers, PAUSE_KEY.0) != 0;
                let _ = started.send((GetCurrentThreadId(), stop && pause));
                let mut message: MSG = std::mem::zeroed();
                while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                    if message.message == WM_HOTKEY {
                        let key = if message.wParam == 1 {
                            RecordingKey::Stop
                        } else {
                            RecordingKey::Pause
                        };
                        let _ = sender.send(key);
                        wake();
                    }
                }
                UnregisterHotKey(std::ptr::null_mut(), 1);
                UnregisterHotKey(std::ptr::null_mut(), 2);
            }
        });
        let (thread_id, taken) = ready.recv().unwrap_or((0, false));
        Self {
            thread_id,
            worker: Some(worker),
            taken,
        }
    }
}

impl Drop for RecordingKeys {
    fn drop(&mut self) {
        // SAFETY: the message only asks that thread's loop to end.
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        #[cfg(test)]
        turns::done();
    }
}

/// Keeps every window of the interface's thread out of screen captures, or lets them back in.
/// Windows made later, such as the recording overlay, need this again once they exist.
pub fn hide_from_captures(hidden: bool) {
    unsafe extern "system" fn each(window: HWND, affinity: LPARAM) -> i32 {
        // SAFETY: the window belongs to this thread and is live during the enumeration.
        unsafe {
            SetWindowDisplayAffinity(window, affinity as u32);
        }
        1
    }
    let affinity = if hidden {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    // SAFETY: the callback only runs during this call.
    unsafe {
        EnumThreadWindows(GetCurrentThreadId(), Some(each), affinity as LPARAM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_can_be_taken_and_let_go() {
        let (sender, _keys) = mpsc::channel();
        let first = RecordingKeys::listen(sender.clone(), || {});
        assert!(first.taken);
        drop(first);
        // Let go, so they can be taken again.
        let again = RecordingKeys::listen(sender, || {});
        assert!(again.taken);
    }

    #[test]
    fn hiding_with_no_windows_does_nothing() {
        hide_from_captures(true);
        hide_from_captures(false);
    }
}
