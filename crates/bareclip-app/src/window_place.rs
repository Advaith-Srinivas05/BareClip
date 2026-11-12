//! Where the main window was when it last closed, so the next run opens it there.
//!
//! Windows keeps a window's normal size and spot even while it is maximized or minimized, which
//! is what un-maximizing goes back to, so that is what gets saved and put back.

use serde::{Deserialize, Serialize};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, RECT},
    Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
        MonitorFromWindow,
    },
    UI::WindowsAndMessaging::{
        GetWindowPlacement, GetWindowRect, SW_HIDE, SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED,
        SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPlacement, SetWindowPos, WINDOWPLACEMENT,
        WPF_RESTORETOMAXIMIZED,
    },
};

/// How much of the top of the window counts as somewhere to grab it, in pixels.
const GRAB_HEIGHT: i32 = 40;
/// How much of that has to be on a screen for the window to count as reachable.
const GRAB_WIDTH: i32 = 160;

/// The window's normal size and spot, in the numbers Windows gives for it, and whether it was
/// maximized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowPlace {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub maximized: bool,
}

/// A rectangle on the desktop, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Area {
    fn width(self) -> i32 {
        self.right - self.left
    }

    fn height(self) -> i32 {
        self.bottom - self.top
    }

    fn overlap(self, other: Area) -> Option<Area> {
        let area = Area {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (area.width() > 0 && area.height() > 0).then_some(area)
    }

    fn overlap_size(self, other: Area) -> i64 {
        self.overlap(other)
            .map_or(0, |area| i64::from(area.width()) * i64::from(area.height()))
    }
}

impl From<RECT> for Area {
    fn from(rect: RECT) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }
}

/// Whether enough of the window's top edge is on a screen to drag it by.
pub fn reachable(window: Area, screens: &[Area]) -> bool {
    let top_edge = Area {
        bottom: window.top + GRAB_HEIGHT,
        ..window
    };
    let wide_enough = GRAB_WIDTH.min(window.width());
    screens.iter().any(|screen| {
        top_edge.overlap(*screen).is_some_and(|on_screen| {
            on_screen.width() >= wide_enough && on_screen.height() >= GRAB_HEIGHT / 2
        })
    })
}

/// The window made no bigger than the screen and moved fully onto it if it had to shrink, as
/// happens after the screen's resolution goes down.
pub fn fit(window: Area, screen: Area) -> Area {
    if window.width() <= screen.width() && window.height() <= screen.height() {
        return window;
    }
    let width = window.width().min(screen.width());
    let height = window.height().min(screen.height());
    let left = window.left.clamp(screen.left, screen.right - width);
    let top = window.top.clamp(screen.top, screen.bottom - height);
    Area {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// A window of this size in the middle of the screen.
pub fn centered(width: i32, height: i32, screen: Area) -> Area {
    let width = width.min(screen.width());
    let height = height.min(screen.height());
    let left = screen.left + (screen.width() - width) / 2;
    let top = screen.top + (screen.height() - height) / 2;
    Area {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// Where the window is now, to save for next time.
pub fn read(window: HWND) -> Option<WindowPlace> {
    let placement = placement(window)?;
    let normal = placement.rcNormalPosition;
    let maximized = placement.showCmd == SW_SHOWMAXIMIZED as u32
        || (placement.showCmd == SW_SHOWMINIMIZED as u32
            && placement.flags & WPF_RESTORETOMAXIMIZED != 0);
    Some(WindowPlace {
        left: normal.left,
        top: normal.top,
        right: normal.right,
        bottom: normal.bottom,
        maximized,
    })
}

/// Puts the still-hidden window back where it was saved, or in the middle of its screen if
/// nothing was saved or that spot is no longer on any screen. Says whether to maximize it,
/// which is left to the window's own code so it knows.
pub fn restore(window: HWND, saved: Option<WindowPlace>) -> bool {
    let Some(before) = placement(window) else {
        return false;
    };
    let screens = screens();
    if let Some(saved) = saved
        && let Some(on_screen) = saved_on_screen(window, &before, saved)
        && reachable(on_screen, &screens)
    {
        let mut wanted = before;
        wanted.flags = 0;
        wanted.showCmd = SW_HIDE as u32;
        wanted.rcNormalPosition = RECT {
            left: saved.left,
            top: saved.top,
            right: saved.right,
            bottom: saved.bottom,
        };
        // Moving onto a screen with another scale makes Windows rescale the window on the way,
        // so the second time lands it at the saved size.
        unsafe {
            SetWindowPlacement(window, &wanted);
            SetWindowPlacement(window, &wanted);
        }
        if let Some(now) = window_area(window)
            && let Some(screen) = screens
                .iter()
                .copied()
                .max_by_key(|screen| now.overlap_size(*screen))
        {
            let fitted = fit(now, screen);
            if fitted != now {
                move_to(window, fitted);
            }
        }
        return saved.maximized;
    }
    if let (Some(now), Some(screen)) = (window_area(window), screen_of(window)) {
        move_to(window, centered(now.width(), now.height(), screen));
    }
    saved.is_some_and(|saved| saved.maximized)
}

/// Where the saved spot is on the desktop. Windows saves it measured from the edge of the
/// taskbar when the taskbar is at the top or left, which the new window's own spot shows. It
/// has to be worked out before moving there, because Windows quietly pulls a window that would
/// be on no screen back onto one.
fn saved_on_screen(window: HWND, before: &WINDOWPLACEMENT, saved: WindowPlace) -> Option<Area> {
    let now = window_area(window)?;
    let dx = now.left - before.rcNormalPosition.left;
    let dy = now.top - before.rcNormalPosition.top;
    Some(Area {
        left: saved.left + dx,
        top: saved.top + dy,
        right: saved.right + dx,
        bottom: saved.bottom + dy,
    })
}

fn placement(window: HWND) -> Option<WINDOWPLACEMENT> {
    let mut placement: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
    placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    (unsafe { GetWindowPlacement(window, &mut placement) } != 0).then_some(placement)
}

fn window_area(window: HWND) -> Option<Area> {
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    (unsafe { GetWindowRect(window, &mut rect) } != 0).then(|| rect.into())
}

fn move_to(window: HWND, area: Area) {
    unsafe {
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            area.left,
            area.top,
            area.width(),
            area.height(),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// The part of each screen windows can use, which leaves out the taskbar.
fn screens() -> Vec<Area> {
    unsafe extern "system" fn visit(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        found: LPARAM,
    ) -> windows_sys::core::BOOL {
        let found = unsafe { &mut *(found as *mut Vec<Area>) };
        if let Some(area) = work_area(monitor) {
            found.push(area);
        }
        1
    }
    let mut found: Vec<Area> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(visit),
            &mut found as *mut _ as LPARAM,
        );
    }
    found
}

/// The screen the window is mostly on, or the main one.
fn screen_of(window: HWND) -> Option<Area> {
    work_area(unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTOPRIMARY) })
}

fn work_area(monitor: HMONITOR) -> Option<Area> {
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0).then(|| info.rcWork.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WS_OVERLAPPEDWINDOW,
    };

    const SCREEN: Area = Area {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };
    const RIGHT_SCREEN: Area = Area {
        left: 1920,
        top: 0,
        right: 3840,
        bottom: 1040,
    };

    fn area(left: i32, top: i32, width: i32, height: i32) -> Area {
        Area {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    #[test]
    fn a_window_on_the_screen_is_reachable() {
        assert!(reachable(area(100, 100, 1280, 800), &[SCREEN]));
    }

    #[test]
    fn a_window_hanging_off_the_bottom_or_side_is_still_reachable() {
        assert!(reachable(area(100, 900, 1280, 800), &[SCREEN]));
        assert!(reachable(area(1700, 100, 1280, 800), &[SCREEN]));
        assert!(reachable(area(-1100, 100, 1280, 800), &[SCREEN]));
    }

    #[test]
    fn a_window_on_a_screen_that_was_unplugged_is_not_reachable() {
        let on_the_right = area(2200, 100, 1280, 800);

        assert!(reachable(on_the_right, &[SCREEN, RIGHT_SCREEN]));
        assert!(!reachable(on_the_right, &[SCREEN]));
    }

    #[test]
    fn a_window_whose_top_is_above_the_screen_or_mostly_off_it_is_not_reachable() {
        assert!(!reachable(area(100, -30, 1280, 800), &[SCREEN]));
        assert!(!reachable(area(1860, 100, 1280, 800), &[SCREEN]));
        assert!(!reachable(area(100, 1030, 1280, 800), &[SCREEN]));
    }

    #[test]
    fn a_window_across_two_screens_is_reachable() {
        assert!(reachable(
            area(1500, 100, 1280, 800),
            &[SCREEN, RIGHT_SCREEN]
        ));
    }

    #[test]
    fn a_window_that_fits_is_left_alone() {
        let window = area(1700, 900, 1280, 800);

        assert_eq!(fit(window, SCREEN), window);
    }

    #[test]
    fn a_window_bigger_than_the_screen_shrinks_onto_it() {
        let small_screen = area(0, 0, 1366, 728);

        assert_eq!(fit(area(200, 100, 1900, 1000), small_screen), small_screen);
        assert_eq!(
            fit(area(200, 100, 1300, 1000), small_screen),
            area(66, 0, 1300, 728)
        );
    }

    #[test]
    fn centering_uses_the_middle_of_the_part_windows_can_use() {
        assert_eq!(centered(1280, 800, SCREEN), area(320, 120, 1280, 800));
        assert_eq!(
            centered(1280, 800, RIGHT_SCREEN),
            area(2240, 120, 1280, 800)
        );
        let below_a_top_taskbar = area(0, 48, 1920, 1032);
        assert_eq!(
            centered(1280, 800, below_a_top_taskbar),
            area(320, 164, 1280, 800)
        );
    }

    #[test]
    fn centering_a_window_bigger_than_the_screen_fills_it() {
        let small_screen = area(0, 0, 1366, 728);

        assert_eq!(centered(1920, 1080, small_screen), small_screen);
    }

    /// A hidden window of Windows' own, to put somewhere and read back.
    struct HiddenWindow(HWND);

    impl HiddenWindow {
        fn new() -> Self {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let window = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    400,
                    300,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            assert!(!window.is_null());
            Self(window)
        }
    }

    impl Drop for HiddenWindow {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) };
        }
    }

    /// A spot on the main screen, wherever the test runs.
    fn spot_on_the_main_screen(window: HWND) -> WindowPlace {
        let screen = screen_of(window).unwrap();
        WindowPlace {
            left: screen.left + 10,
            top: screen.top + 20,
            right: screen.left + 10 + 300,
            bottom: screen.top + 20 + 200,
            maximized: false,
        }
    }

    #[test]
    fn a_saved_spot_is_put_back_and_read_the_same() {
        let window = HiddenWindow::new();
        let saved = spot_on_the_main_screen(window.0);

        assert!(!restore(window.0, Some(saved)));
        assert_eq!(read(window.0), Some(saved));
    }

    #[test]
    fn a_maximized_spot_is_put_back_and_asks_to_be_maximized() {
        let window = HiddenWindow::new();
        let normal = spot_on_the_main_screen(window.0);
        let saved = WindowPlace {
            maximized: true,
            ..normal
        };

        assert!(restore(window.0, Some(saved)));
        assert_eq!(read(window.0), Some(normal));
    }

    #[test]
    fn a_spot_with_its_top_above_the_screen_falls_back_to_the_middle() {
        let window = HiddenWindow::new();
        let screen = screen_of(window.0).unwrap();
        let top_cut_off = WindowPlace {
            top: screen.top - 150,
            bottom: screen.top + 50,
            ..spot_on_the_main_screen(window.0)
        };

        assert!(!restore(window.0, Some(top_cut_off)));
        assert_eq!(window_area(window.0), Some(centered(400, 300, screen)));
    }

    #[test]
    fn restoring_leaves_the_window_hidden() {
        use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;
        let window = HiddenWindow::new();
        let lost = WindowPlace {
            left: -40_000,
            top: -40_000,
            right: -39_000,
            bottom: -39_000,
            maximized: false,
        };

        restore(window.0, Some(spot_on_the_main_screen(window.0)));
        restore(window.0, Some(lost));
        restore(window.0, None);
        assert_eq!(unsafe { IsWindowVisible(window.0) }, 0);
    }

    #[test]
    fn with_nothing_saved_the_window_is_centered_at_its_own_size() {
        let window = HiddenWindow::new();
        let screen = screen_of(window.0).unwrap();

        assert!(!restore(window.0, None));
        assert_eq!(window_area(window.0), Some(centered(400, 300, screen)));
    }

    #[test]
    fn a_spot_on_no_screen_falls_back_to_the_middle_at_the_starting_size() {
        let window = HiddenWindow::new();
        let screen = screen_of(window.0).unwrap();
        let lost = WindowPlace {
            left: -40_000,
            top: -40_000,
            right: -40_000 + 900,
            bottom: -40_000 + 700,
            maximized: true,
        };

        assert!(restore(window.0, Some(lost)));
        assert_eq!(window_area(window.0), Some(centered(400, 300, screen)));
    }
}
