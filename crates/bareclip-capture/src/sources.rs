//! What can be recorded: each screen, and each window someone could pick.

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, RECT},
        Graphics::{
            Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
            Gdi::{
                EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
            },
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GWL_EXSTYLE, GetWindowLongW, GetWindowRect, GetWindowTextLengthW,
            GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_TOOLWINDOW,
        },
    },
    core::BOOL,
};

/// Something to record, by the handle Windows knows it by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Screen(isize),
    Window(isize),
}

/// A source as it is offered to pick from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offered {
    pub source: Source,
    pub name: String,
    pub size: (u32, u32),
    /// The main screen, which starts chosen.
    pub is_main: bool,
}

const MONITORINFOF_PRIMARY: u32 = 1;

fn rect_size(rect: RECT) -> (u32, u32) {
    (
        (rect.right - rect.left).max(0) as u32,
        (rect.bottom - rect.top).max(0) as u32,
    )
}

/// Every screen, the main one first, named "Screen 1" and so on in Windows' order.
pub fn screens() -> Vec<Offered> {
    unsafe extern "system" fn each(monitor: HMONITOR, _: HDC, _: *mut RECT, found: LPARAM) -> BOOL {
        // SAFETY: `found` is the vector `screens` passed in, alive for the whole enumeration.
        let found = unsafe { &mut *(found.0 as *mut Vec<(HMONITOR, MONITORINFOEXW)>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of_val(&info) as u32;
        // SAFETY: the structure is the right size, as its first field says.
        let read = unsafe { GetMonitorInfoW(monitor, (&raw mut info).cast::<MONITORINFO>()) };
        if read.as_bool() {
            found.push((monitor, info));
        }
        BOOL(1)
    }
    let mut found: Vec<(HMONITOR, MONITORINFOEXW)> = Vec::new();
    // SAFETY: the callback only runs during this call, while `found` is alive.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM((&raw mut found) as isize));
    }
    let mut offered: Vec<Offered> = found
        .into_iter()
        .enumerate()
        .map(|(index, (monitor, info))| {
            let rect = info.monitorInfo.rcMonitor;
            let (width, height) = rect_size(rect);
            let is_main = info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;
            Offered {
                source: Source::Screen(monitor.0 as isize),
                name: format!(
                    "Screen {}{}, {width} x {height}",
                    index + 1,
                    if is_main { " (main)" } else { "" }
                ),
                size: (width, height),
                is_main,
            }
        })
        .collect();
    offered.sort_by_key(|screen| !screen.is_main);
    offered
}

/// Windows that show on the taskbar and can be recorded, leaving out this program's own, which
/// hide themselves from recordings anyway.
pub fn windows() -> Vec<Offered> {
    unsafe extern "system" fn each(window: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` is the vector `windows` passed in, alive for the whole enumeration.
        let found = unsafe { &mut *(found.0 as *mut Vec<Offered>) };
        if let Some(offered) = offer(window) {
            found.push(offered);
        }
        BOOL(1)
    }
    let mut found: Vec<Offered> = Vec::new();
    // SAFETY: the callback only runs during this call, while `found` is alive.
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM((&raw mut found) as isize));
    }
    found.sort_by_key(|window| window.name.to_lowercase());
    found
}

/// A window as it would be offered, if it should be.
fn offer(window: HWND) -> Option<Offered> {
    // SAFETY: each call only reads about a window handle Windows just gave us. A window that has
    // gone since simply reads as nothing.
    unsafe {
        if !IsWindowVisible(window).as_bool() || IsIconic(window).as_bool() {
            return None;
        }
        if GetWindowLongW(window, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }
        let mut cloaked = 0_u32;
        let read = DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        );
        if read.is_ok() && cloaked != 0 {
            return None;
        }
        let mut process = 0;
        GetWindowThreadProcessId(window, Some(&mut process));
        if process == std::process::id() {
            return None;
        }
        let length = GetWindowTextLengthW(window);
        if length <= 0 {
            return None;
        }
        let mut text = vec![0_u16; length as usize + 1];
        let copied = GetWindowTextW(window, &mut text);
        let name = String::from_utf16_lossy(&text[..copied.max(0) as usize]);
        if name.trim().is_empty() {
            return None;
        }
        let mut rect = RECT::default();
        GetWindowRect(window, &mut rect).ok()?;
        let size = rect_size(rect);
        if size.0 < 2 || size.1 < 2 {
            return None;
        }
        Some(Offered {
            source: Source::Window(window.0 as isize),
            name,
            size,
            is_main: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_a_main_screen_first() {
        let screens = screens();
        assert!(!screens.is_empty());
        assert!(screens[0].is_main);
        assert!(screens[0].name.contains("(main)"));
        assert!(
            screens
                .iter()
                .all(|screen| screen.size.0 > 0 && screen.size.1 > 0)
        );
    }

    #[test]
    fn windows_offered_all_have_names() {
        for window in windows() {
            assert!(!window.name.trim().is_empty());
            assert!(matches!(window.source, Source::Window(_)));
        }
    }
}
