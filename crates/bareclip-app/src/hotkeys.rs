//! Keys that work while another program has focus, such as a game being recorded, and keeping
//! BareClip's own windows out of what is recorded.

use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
};

use eframe::egui;
use serde::{Deserialize, Serialize};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        Input::KeyboardAndMouse::{
            HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey,
            UnregisterHotKey,
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
    /// Turns the microphone on or off.
    Microphone,
}

/// A key with any of Ctrl, Alt and Shift, as Windows knows it: `key` is its virtual key code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Shortcut {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: u32,
}

/// Every key a shortcut can use: how egui calls it, Windows' code for it, and its name. Escape
/// isn't one, because it backs out of picking a key.
const KEYS: &[(egui::Key, u32, &str)] = {
    use egui::Key;
    &[
        (Key::A, 0x41, "A"),
        (Key::B, 0x42, "B"),
        (Key::C, 0x43, "C"),
        (Key::D, 0x44, "D"),
        (Key::E, 0x45, "E"),
        (Key::F, 0x46, "F"),
        (Key::G, 0x47, "G"),
        (Key::H, 0x48, "H"),
        (Key::I, 0x49, "I"),
        (Key::J, 0x4A, "J"),
        (Key::K, 0x4B, "K"),
        (Key::L, 0x4C, "L"),
        (Key::M, 0x4D, "M"),
        (Key::N, 0x4E, "N"),
        (Key::O, 0x4F, "O"),
        (Key::P, 0x50, "P"),
        (Key::Q, 0x51, "Q"),
        (Key::R, 0x52, "R"),
        (Key::S, 0x53, "S"),
        (Key::T, 0x54, "T"),
        (Key::U, 0x55, "U"),
        (Key::V, 0x56, "V"),
        (Key::W, 0x57, "W"),
        (Key::X, 0x58, "X"),
        (Key::Y, 0x59, "Y"),
        (Key::Z, 0x5A, "Z"),
        (Key::Num0, 0x30, "0"),
        (Key::Num1, 0x31, "1"),
        (Key::Num2, 0x32, "2"),
        (Key::Num3, 0x33, "3"),
        (Key::Num4, 0x34, "4"),
        (Key::Num5, 0x35, "5"),
        (Key::Num6, 0x36, "6"),
        (Key::Num7, 0x37, "7"),
        (Key::Num8, 0x38, "8"),
        (Key::Num9, 0x39, "9"),
        (Key::F1, 0x70, "F1"),
        (Key::F2, 0x71, "F2"),
        (Key::F3, 0x72, "F3"),
        (Key::F4, 0x73, "F4"),
        (Key::F5, 0x74, "F5"),
        (Key::F6, 0x75, "F6"),
        (Key::F7, 0x76, "F7"),
        (Key::F8, 0x77, "F8"),
        (Key::F9, 0x78, "F9"),
        (Key::F10, 0x79, "F10"),
        (Key::F11, 0x7A, "F11"),
        (Key::F12, 0x7B, "F12"),
        (Key::F13, 0x7C, "F13"),
        (Key::F14, 0x7D, "F14"),
        (Key::F15, 0x7E, "F15"),
        (Key::F16, 0x7F, "F16"),
        (Key::F17, 0x80, "F17"),
        (Key::F18, 0x81, "F18"),
        (Key::F19, 0x82, "F19"),
        (Key::F20, 0x83, "F20"),
        (Key::F21, 0x84, "F21"),
        (Key::F22, 0x85, "F22"),
        (Key::F23, 0x86, "F23"),
        (Key::F24, 0x87, "F24"),
        (Key::Space, 0x20, "Space"),
        (Key::Enter, 0x0D, "Enter"),
        (Key::Tab, 0x09, "Tab"),
        (Key::Backspace, 0x08, "Backspace"),
        (Key::Insert, 0x2D, "Insert"),
        (Key::Delete, 0x2E, "Delete"),
        (Key::Home, 0x24, "Home"),
        (Key::End, 0x23, "End"),
        (Key::PageUp, 0x21, "Page Up"),
        (Key::PageDown, 0x22, "Page Down"),
        (Key::ArrowLeft, 0x25, "Left"),
        (Key::ArrowUp, 0x26, "Up"),
        (Key::ArrowRight, 0x27, "Right"),
        (Key::ArrowDown, 0x28, "Down"),
        (Key::Minus, 0xBD, "-"),
        (Key::Equals, 0xBB, "="),
        (Key::Plus, 0xBB, "="),
        (Key::Comma, 0xBC, ","),
        (Key::Period, 0xBE, "."),
        (Key::Semicolon, 0xBA, ";"),
        (Key::Slash, 0xBF, "/"),
        (Key::Backslash, 0xDC, "\\"),
        (Key::OpenBracket, 0xDB, "["),
        (Key::CloseBracket, 0xDD, "]"),
        (Key::Quote, 0xDE, "'"),
        (Key::Backtick, 0xC0, "`"),
    ]
};

/// Windows' first and last function keys.
const F1_KEY: u32 = 0x70;
const F24_KEY: u32 = 0x87;

impl Shortcut {
    /// Ctrl+Alt+Shift with a letter, which no game or program is likely to use.
    pub const STOP: Self = Self::with_all(b'R' as u32);
    pub const PAUSE: Self = Self::with_all(b'P' as u32);
    pub const MICROPHONE: Self = Self::with_all(b'M' as u32);

    const fn with_all(key: u32) -> Self {
        Self {
            ctrl: true,
            alt: true,
            shift: true,
            key,
        }
    }

    /// The shortcut for a key pressed in BareClip's window, if it's one a shortcut can use.
    pub fn from_pressed(key: egui::Key, modifiers: egui::Modifiers) -> Option<Self> {
        let (_, code, _) = KEYS.iter().find(|(known, _, _)| *known == key)?;
        Some(Self {
            ctrl: modifiers.ctrl,
            alt: modifiers.alt,
            shift: modifiers.shift,
            key: *code,
        })
    }

    /// How it's written, such as `Ctrl+Alt+Shift+R`.
    pub fn name(self) -> String {
        let key = KEYS
            .iter()
            .find(|(_, code, _)| *code == self.key)
            .map_or_else(
                || format!("Key {}", self.key),
                |(_, _, name)| (*name).to_owned(),
            );
        let mut parts: Vec<String> = [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
        ]
        .iter()
        .filter(|(held, _)| *held)
        .map(|(_, name)| (*name).to_owned())
        .collect();
        parts.push(key);
        parts.join("+")
    }

    /// Whether it's an ordinary key that games and typing use, held with nothing but Shift at
    /// most. Function keys count as unusual enough on their own.
    pub fn is_plain(self) -> bool {
        !self.ctrl && !self.alt && !(F1_KEY..=F24_KEY).contains(&self.key)
    }

    fn modifiers(self) -> HOT_KEY_MODIFIERS {
        let mut modifiers = MOD_NOREPEAT;
        for (held, flag) in [
            (self.ctrl, MOD_CONTROL),
            (self.alt, MOD_ALT),
            (self.shift, MOD_SHIFT),
        ] {
            if held {
                modifiers |= flag;
            }
        }
        modifiers
    }

    /// Whether no other program holds it. Windows is asked to hand it over and it's let straight
    /// go, on a thread of its own so nothing of the interface's is touched.
    pub fn is_free(self) -> bool {
        #[cfg(test)]
        turns::wait();
        let free = thread::spawn(move || {
            // SAFETY: the key belongs to this thread only while it's checked.
            unsafe {
                let taken =
                    RegisterHotKey(std::ptr::null_mut(), 1, self.modifiers(), self.key) != 0;
                if taken {
                    UnregisterHotKey(std::ptr::null_mut(), 1);
                }
                taken
            }
        })
        .join()
        .unwrap_or(false);
        #[cfg(test)]
        turns::done();
        free
    }
}

/// Why a shortcut can't be used for one of the recording keys, or `None` if it can. `current` is
/// the one it replaces, and `others` the other recording keys.
pub fn shortcut_problem(
    wanted: Shortcut,
    current: Shortcut,
    others: &[Shortcut],
) -> Option<String> {
    if wanted == current {
        return None;
    }
    if others.contains(&wanted) {
        return Some(format!(
            "{} is already another recording key.",
            wanted.name()
        ));
    }
    (!wanted.is_free()).then(|| {
        format!(
            "Another program already uses {}, so pick another.",
            wanted.name()
        )
    })
}

/// The keys, held for as long as this lives. Each press is sent down the channel, and `wake` is
/// called so the interface looks at it straight away.
pub struct RecordingKeys {
    thread_id: u32,
    worker: Option<JoinHandle<()>>,
    /// Whether Windows let every key be taken. Another program may already hold one.
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
    /// Takes the keys, each for what it asks for.
    pub fn listen(
        keys: [(RecordingKey, Shortcut); 3],
        sender: mpsc::Sender<RecordingKey>,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        #[cfg(test)]
        turns::wait();
        let (started, ready) = mpsc::channel();
        let worker = thread::spawn(move || {
            // SAFETY: the keys belong to this thread, which takes its own messages until told to
            // stop, then lets the keys go. Each is known by its place in `keys`, counted from 1.
            unsafe {
                let mut all_taken = true;
                for (id, (_, shortcut)) in (1..).zip(keys) {
                    all_taken &= RegisterHotKey(
                        std::ptr::null_mut(),
                        id,
                        shortcut.modifiers(),
                        shortcut.key,
                    ) != 0;
                }
                let _ = started.send((GetCurrentThreadId(), all_taken));
                let mut message: MSG = std::mem::zeroed();
                while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                    if message.message == WM_HOTKEY
                        && let Some((key, _)) = message
                            .wParam
                            .checked_sub(1)
                            .and_then(|index| keys.get(index))
                    {
                        let _ = sender.send(*key);
                        wake();
                    }
                }
                for id in (1..).take(keys.len()) {
                    UnregisterHotKey(std::ptr::null_mut(), id);
                }
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
        let keys = usual_keys();
        let first = RecordingKeys::listen(keys, sender.clone(), || {});
        assert!(first.taken);
        drop(first);
        // Let go, so they can be taken again.
        let again = RecordingKeys::listen(keys, sender, || {});
        assert!(again.taken);
    }

    fn usual_keys() -> [(RecordingKey, Shortcut); 3] {
        [
            (RecordingKey::Stop, Shortcut::STOP),
            (RecordingKey::Pause, Shortcut::PAUSE),
            (RecordingKey::Microphone, Shortcut::MICROPHONE),
        ]
    }

    #[test]
    fn changed_keys_are_the_ones_taken() {
        let (sender, _keys) = mpsc::channel();
        let stop = Shortcut {
            ctrl: true,
            alt: true,
            shift: true,
            key: 0x86,
        };
        let mut keys = usual_keys();
        keys[0].1 = stop;
        let held = RecordingKeys::listen(keys, sender, || {});
        assert!(held.taken);
        // Another program would now find it taken, and the old stop key free.
        let checked = thread::spawn(move || {
            // SAFETY: the keys belong to this thread only while they're checked.
            unsafe {
                let new = RegisterHotKey(std::ptr::null_mut(), 9, stop.modifiers(), stop.key) != 0;
                let old = RegisterHotKey(
                    std::ptr::null_mut(),
                    10,
                    Shortcut::STOP.modifiers(),
                    Shortcut::STOP.key,
                ) != 0;
                UnregisterHotKey(std::ptr::null_mut(), 9);
                UnregisterHotKey(std::ptr::null_mut(), 10);
                (new, old)
            }
        })
        .join()
        .expect("it answers");
        assert_eq!(checked, (false, true));
    }

    #[test]
    fn hiding_with_no_windows_does_nothing() {
        hide_from_captures(true);
        hide_from_captures(false);
    }

    #[test]
    fn a_shortcut_is_written_the_way_people_say_it() {
        assert_eq!(Shortcut::STOP.name(), "Ctrl+Alt+Shift+R");
        assert_eq!(Shortcut::PAUSE.name(), "Ctrl+Alt+Shift+P");
        assert_eq!(Shortcut::MICROPHONE.name(), "Ctrl+Alt+Shift+M");
        let pressed = |key, modifiers| Shortcut::from_pressed(key, modifiers).map(Shortcut::name);
        assert_eq!(
            pressed(egui::Key::F9, egui::Modifiers::NONE).as_deref(),
            Some("F9")
        );
        assert_eq!(
            pressed(egui::Key::Num5, egui::Modifiers::CTRL).as_deref(),
            Some("Ctrl+5")
        );
        assert_eq!(
            pressed(
                egui::Key::PageDown,
                egui::Modifiers::ALT | egui::Modifiers::SHIFT
            )
            .as_deref(),
            Some("Alt+Shift+Page Down")
        );
        assert_eq!(pressed(egui::Key::Escape, egui::Modifiers::NONE), None);
    }

    #[test]
    fn plain_keys_are_the_ones_games_and_typing_use() {
        let pressed = |key, modifiers| Shortcut::from_pressed(key, modifiers).expect("a key");
        assert!(pressed(egui::Key::R, egui::Modifiers::NONE).is_plain());
        assert!(pressed(egui::Key::R, egui::Modifiers::SHIFT).is_plain());
        assert!(pressed(egui::Key::Space, egui::Modifiers::NONE).is_plain());
        assert!(pressed(egui::Key::Num1, egui::Modifiers::NONE).is_plain());
        assert!(!pressed(egui::Key::R, egui::Modifiers::CTRL).is_plain());
        assert!(!pressed(egui::Key::R, egui::Modifiers::ALT).is_plain());
        assert!(!pressed(egui::Key::F9, egui::Modifiers::NONE).is_plain());
        assert!(!pressed(egui::Key::F24, egui::Modifiers::SHIFT).is_plain());
    }

    #[test]
    fn every_key_windows_knows_has_a_name() {
        for (_, code, name) in KEYS {
            let shortcut = Shortcut {
                ctrl: false,
                alt: false,
                shift: false,
                key: *code,
            };
            assert_eq!(shortcut.name(), *name);
        }
    }

    #[test]
    fn a_key_held_by_another_program_is_refused() {
        // Something else takes a key, as another program would.
        let held = Shortcut {
            ctrl: true,
            alt: true,
            shift: true,
            key: 0x87,
        };
        let (holding, held_now) = mpsc::channel();
        let (done, finish) = mpsc::channel::<()>();
        let holder = thread::spawn(move || {
            // SAFETY: the key belongs to this thread until it lets it go.
            unsafe {
                let taken =
                    RegisterHotKey(std::ptr::null_mut(), 7, held.modifiers(), held.key) != 0;
                let _ = holding.send(taken);
                let _ = finish.recv();
                UnregisterHotKey(std::ptr::null_mut(), 7);
            }
        });
        assert!(held_now.recv().expect("it answers"), "the key was free");
        let problem = shortcut_problem(held, Shortcut::STOP, &[Shortcut::PAUSE]);
        assert_eq!(
            problem.as_deref(),
            Some("Another program already uses Ctrl+Alt+Shift+F24, so pick another.")
        );
        let _ = done.send(());
        let _ = holder.join();
        assert_eq!(
            shortcut_problem(held, Shortcut::STOP, &[Shortcut::PAUSE]),
            None
        );
    }

    #[test]
    fn no_two_keys_can_be_the_same() {
        let others = [Shortcut::PAUSE, Shortcut::MICROPHONE];
        for taken in others {
            let problem = shortcut_problem(taken, Shortcut::STOP, &others);
            assert_eq!(
                problem,
                Some(format!(
                    "{} is already another recording key.",
                    taken.name()
                ))
            );
        }
        assert_eq!(
            shortcut_problem(Shortcut::STOP, Shortcut::STOP, &others),
            None,
            "keeping the same key is fine"
        );
    }
}
